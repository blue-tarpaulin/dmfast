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
use crate::deps::Dep;
use crate::feature::{Ctx, Feature, Host, Kind, State, Status};
use crate::image::Image;
pub(crate) use report::mimalloc;
use report::summary;
pub use report::{print_stats, stats, verify};
use select::{check_keys, disabled, selection, wanted};

/// What the driver installs and how it reports it, handed in from the loader entry.
pub struct Config {
    /// Feature lists; a program gets those whose hosts include it, in this order.
    pub lists: &'static [&'static [&'static Feature]],
    /// This build's name on the stats line (`DMFAST <version>`).
    pub version: &'static str,
    /// Called with the counts once every feature is installed, still at install time.
    pub after_install: unsafe fn(Counts),
    /// This OS's dependency guards ([`crate::deps`]).
    pub deps: &'static [Dep],
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
    crate::deps::check(config.deps, &core);
    // SAFETY: install time (caller).
    unsafe { run(config, host, Ctx::new(core, Image::host())) };
}

/// Install `config`'s features of `host` into `ctx`'s images (their code copied first), then call
/// [`Config::after_install`] and print the stats line.
///
/// # Safety
/// Install time: no other thread runs BYOND's code; deps checked.
unsafe fn run(config: &'static Config, host: Host, ctx: Ctx) {
    let _ = INSTALLED.set((config, host));
    crate::snapshot::take(ctx.core());
    if let Ok(exe) = ctx.exe() {
        crate::snapshot::take(exe);
    }
    let entries = selection();
    let build = byond(ctx.core());
    let (mut offered, mut applied, mut fixes) = (0, 0, 0);
    for f in config.ordered(host) {
        let Some(install) = f.install.run else { continue };
        let want = wanted(f, entries);
        // Leave `f` out with `state`, saying why under `DMFAST_STATS` if it was wanted.
        let skip = |why: std::fmt::Arguments<'_>, state: State| {
            if want && stats() {
                eprintln!("dmfast: disabled {}{} - {why}", f.kind_prefix(), f.key);
            }
            f.set_state(state);
        };
        if let Some((fixed, _)) = f.byond.fixed_in
            && build >= fixed
        {
            skip(format_args!("not needed: BYOND fixed it in {}", build_name(fixed)), State::OutOfRange);
            continue;
        }
        if build != 0 && build < f.byond.since {
            skip(format_args!("needs BYOND {} or later", build_name(f.byond.since)), State::OutOfRange);
            continue;
        }
        if let Status::Deprecated(why) = f.status {
            skip(format_args!("deprecated: {why}"), State::Deprecated);
            continue;
        }
        if f.offered.is_some_and(|o| !o(&ctx)) {
            continue;
        }
        if f.kind == Kind::Perf {
            offered += 1;
        }
        if !want {
            f.set_state(State::Skipped);
            continue;
        }
        if disabled(f.key) {
            if stats() {
                match f.kind {
                    Kind::Perf => eprintln!("dmfast: {}: skipped (DMFAST_DISABLE)", f.title),
                    Kind::Fix => eprintln!("dmfast: fix {} ({}): skipped (DMFAST_DISABLE)", f.key, f.title),
                }
            }
            f.set_state(State::Disabled);
            continue;
        }
        let result = if let Some(r) = f.requires.iter().find(|r| !r.applied()) {
            Err(format!("needs {}", r.key).into())
        } else if crate::deps::missing(f.key) {
            Err("mismatched signature".into())
        } else {
            crate::ledger::begin();
            match install(&ctx) {
                // SAFETY: install time (caller).
                Ok(detail) => unsafe { crate::ledger::commit(f) }.map(|()| detail),
                Err(e) => {
                    crate::ledger::discard();
                    Err(e)
                }
            }
        };
        match result {
            Ok(detail) => {
                f.set_state(State::Applied);
                match f.kind {
                    Kind::Perf => applied += 1,
                    Kind::Fix => fixes += 1,
                }
                if stats() {
                    match f.kind {
                        Kind::Perf => eprintln!("dmfast: {}: {detail}", f.title),
                        Kind::Fix => eprintln!("dmfast: fix {}: {}: {detail}", f.key, f.title),
                    }
                }
            }
            Err(e) => {
                f.set_state(State::Failed);
                if f.kind == Kind::Fix || stats() {
                    eprintln!("dmfast: disabled {}{} - {e}", f.kind_prefix(), f.key);
                }
            }
        }
    }
    // Images, and the `Ctx::shared` values holding them, go before the copy is released.
    drop(ctx);
    crate::image::close();
    // SAFETY: install is over and images closed: nothing holds or makes a slice of the copy.
    unsafe { crate::snapshot::release() };
    check_keys(config);
    // SAFETY: still install time; `after_install` is the loader entry's own hook.
    unsafe { (config.after_install)(Counts { applied, offered, fixes }) };
    if stats() {
        let program = if host == Host::Server { " (runtime)" } else { "" };
        eprintln!("dmfast: {}{program}, {}, {}", config.version, summary(config, host), crate::alloc_status());
        #[cfg(target_os = "linux")]
        // SAFETY: registers a plain `extern "C" fn()` that captures nothing.
        unsafe {
            libc::atexit(report::print_stats_at_exit)
        };
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
