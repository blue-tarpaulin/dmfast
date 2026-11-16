//! Selecting, ordering and installing features ([`crate::feature`]), and reporting them: `DMFAST_DISABLE`
//! and the fix selection (`DMFAST_FIXES` / `dmfast.ini`, `select`), `DMFAST_STATS` (`report`).
//!
//! A feature installs if it is on by default or selected, not turned off (`-key` or `DMFAST_DISABLE`), meant for
//! the running BYOND build, not deprecated, its required features applied and its dependency guards
//! ([`crate::deps`]) passed. Order: list order, each after what it names in `after` / `requires`.
//!
//! Every feature sees the code as shipped (`crate::snapshot`); [`crate::ledger`] checks conflicts.

mod report;
pub(crate) mod select;

use std::sync::OnceLock;

use crate::CORE_NAME;
use crate::deps::{Dep, Guard};
use crate::feature::{Ctx, Feature, Host, InstallFn, Kind, Status};
use crate::image::Image;
pub(crate) use report::mimalloc;
pub use report::{Counter, print_stats, stats, verify};
use report::{read_verify, summary};
use select::{check_keys, disabled, selection, wanted};

/// What the driver installs and how it reports it, handed in from the loader entry.
pub struct Config {
    /// Feature lists; a program gets those whose hosts include it, in this order.
    pub lists: &'static [&'static [&'static Feature]],
    /// This build's name on the stats line (`DMFAST <version>`).
    pub version: &'static str,
    /// This OS's dependency guard tables ([`crate::deps`]): the public patches', and any others'.
    pub deps: &'static [&'static [Dep]],
}

/// What `run` installed in this program.
#[derive(Clone, Copy, Debug)]
pub struct Counts {
    /// Speed patches applied.
    pub applied: u32,
    /// Speed patches this program offers.
    pub offered: u32,
    /// Fixes applied.
    pub fixes: u32,
}

impl Config {
    /// Every feature, of every program and OS.
    pub fn all(&self) -> impl Iterator<Item = &'static Feature> {
        self.lists.iter().copied().flatten().copied()
    }

    /// `host`'s features, each after its `after`/`requires` ones that `host` has, else (or on a cycle) in list order.
    pub(crate) fn ordered(&self, host: Host) -> Vec<&'static Feature> {
        let mut left: Vec<&'static Feature> = self.all().filter(|f| f.hosts.has(host)).collect();
        let mut out = Vec::with_capacity(left.len());
        while !left.is_empty() {
            let waits =
                |f: &Feature| f.after.iter().chain(f.requires).any(|d| left.iter().any(|l| std::ptr::eq(*l, *d)));
            let next = left.iter().position(|f| !waits(f)).unwrap_or(0);
            out.push(left.remove(next));
        }
        out
    }
}

/// The configuration and program this library installed into, once it has.
static INSTALLED: OnceLock<(&'static Config, Host)> = OnceLock::new();

/// Install `config`'s features of `host` into the core library (byondcore.dll / libbyond.so), after its dependency
/// guards.
///
/// # Safety
/// Called from the loader entry (`DllMain`, the constructor): the core library is mapped and relocated, no other
/// thread runs its code.
pub(crate) unsafe fn install(config: &'static Config, host: Host) {
    let core = match Image::loaded(CORE_NAME) {
        Ok(core) => core,
        Err(e) => {
            let what = if host == Host::Compiler {
                CORE_NAME.split_once('.').map_or(CORE_NAME, |(n, _)| n)
            } else {
                "runtime"
            };
            return eprintln!("dmfast: {e}; {what} patches disabled");
        }
    };
    let guard = crate::deps::check(config.deps, &core);
    // SAFETY: install time (caller).
    unsafe { run(config, host, Ctx::new(core, Image::host()), &guard) };
}

/// Install `config`'s features of `host` into `ctx`'s images (their code copied first) except those `guard` failed,
/// then call the applied ones' [`Feature::finish`] and print the stats line.
///
/// # Safety
/// Install time: no other thread runs BYOND's code.
unsafe fn run(config: &'static Config, host: Host, ctx: Ctx, guard: &Guard) {
    let _ = INSTALLED.set((config, host));
    read_verify();
    crate::snapshot::take(ctx.core());
    if let Ok(exe) = ctx.exe() {
        crate::snapshot::take(exe);
    }
    let entries = selection();
    let build = byond(ctx.core());
    let mut counts = Counts { applied: 0, offered: 0, fixes: 0 };
    for f in config.ordered(host) {
        let Some(install) = f.install.0 else { continue };
        let want = wanted(f, entries);
        if let Some(why) = out_of_range(f, build) {
            if want && stats() {
                report::left_out(f, &why);
            }
            continue;
        }
        if f.offered.is_some_and(|o| !o(&ctx)) {
            continue;
        }
        if f.kind == Kind::Perf {
            counts.offered += 1;
        }
        if !want {
            continue;
        }
        if disabled(f.key) {
            report::turned_off(f);
            continue;
        }
        // SAFETY: install time (caller).
        match unsafe { try_install(f, install, &ctx, guard) } {
            Ok(detail) => {
                f.mark_applied();
                match f.kind {
                    Kind::Perf => counts.applied += 1,
                    Kind::Fix => counts.fixes += 1,
                }
                report::applied(f, &detail);
            }
            Err(e) if f.kind == Kind::Fix || stats() => report::left_out(f, &e),
            Err(_) => {}
        }
    }
    // Images, and the `Ctx::shared` values holding them, go before the copy is released.
    drop(ctx);
    crate::image::close();
    // SAFETY: install is over and images closed: nothing holds or makes a slice of the copy.
    unsafe { crate::snapshot::release() };
    check_keys(config);
    for finish in config.all().filter(|f| f.applied()).filter_map(|f| f.finish) {
        finish(counts);
    }
    if stats() {
        let program = if host == Host::Server { " (runtime)" } else { "" };
        eprintln!("dmfast: {}{program}, {}, {}", config.version, summary(config, host), crate::alloc_status());
    }
}

/// Why `f` isn't for BYOND `build` (0: unknown), if it isn't: BYOND fixed the bug, the build is too old, or `f` is
/// deprecated.
fn out_of_range(f: &Feature, build: u32) -> Option<String> {
    if let Some((fixed, _)) = f.byond.fixed_in
        && build >= fixed
    {
        return Some(format!("not needed: BYOND fixed it in {}", build_name(fixed)));
    }
    if build != 0 && build < f.byond.since {
        return Some(format!("needs BYOND {} or later", build_name(f.byond.since)));
    }
    match f.status {
        Status::Deprecated(why) => Some(format!("deprecated: {why}")),
        _ => None,
    }
}

/// Install `f` if what it requires applied and its dependency guards passed; its writes apply only on success.
///
/// # Safety
/// Install time: no other thread runs BYOND's code.
unsafe fn try_install(f: &'static Feature, install: InstallFn, ctx: &Ctx, guard: &Guard) -> crate::Result<String> {
    if let Some(r) = f.requires.iter().find(|r| !r.applied()) {
        return Err(format!("needs {}", r.key).into());
    }
    if guard.failed(f.key) {
        return Err("mismatched signature".into());
    }
    crate::ledger::begin();
    match install(ctx) {
        // SAFETY: install time (caller).
        Ok(detail) => unsafe { crate::ledger::commit(f) }.map(|()| detail),
        Err(e) => {
            crate::ledger::discard();
            Err(e)
        }
    }
}

/// The running BYOND (`version * 10000 + build`, from byondapi's `Byond_GetVersion` in `core`), or 0 if unknown.
fn byond(core: &Image) -> u32 {
    type GetVersion = unsafe extern "C" fn(*mut u32, *mut u32);
    let Ok(f) = core.export(c"Byond_GetVersion") else { return 0 };
    let (mut version, mut build) = (0, 0);
    // SAFETY: byondapi's `Byond_GetVersion(u32 *version, u32 *build)`.
    unsafe { crate::code::Foreign::<GetVersion>::at(f).get()(&mut version, &mut build) };
    version * 10000 + build
}

/// `516.1670` from `516_1670`.
fn build_name(build: u32) -> String {
    format!("{}.{}", build / 10000, build % 10000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{Hosts, Install};

    #[test]
    fn builds_and_deprecation_leave_features_out() {
        const NONE: Install = Install(None);
        let fixed = Feature::perf(Hosts::SERVER, "f", "", NONE).fixed_in(516_1670, "https://x");
        assert_eq!(out_of_range(&fixed, 516_1669), None);
        assert_eq!(out_of_range(&fixed, 516_1670).as_deref(), Some("not needed: BYOND fixed it in 516.1670"));
        let new = Feature::perf(Hosts::SERVER, "n", "", NONE).since(516_1674);
        assert_eq!(out_of_range(&new, 516_1673).as_deref(), Some("needs BYOND 516.1674 or later"));
        assert_eq!(out_of_range(&new, 0), None, "an unknown build doesn't rule it out");
        let old = Feature::perf(Hosts::SERVER, "d", "", NONE).deprecated("gone");
        assert_eq!(out_of_range(&old, 516_1688).as_deref(), Some("deprecated: gone"));
    }
}
