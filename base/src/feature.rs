//! A feature is one speed patch or bug fix: a `static Feature` holding its key, kind, hosts, lifecycle, ordering
//! and per-OS install function, listed in [`crate::driver::Config`]. [`crate::driver`] selects,
//! orders and installs them; nothing else lists features.
//!
//! ```ignore
//! pub(crate) static CRC: Feature =
//!     Feature::perf(Hosts::SERVER, "crc", "CRC-32", install!(windows: windows::install, linux: linux::install))
//!         .stats(print_stats);
//! ```

use std::any::Any;
use std::cell::OnceCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use crate::image::Image;
use crate::{Error, Result};

/// The BYOND program this library was loaded into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    Compiler,
    /// DreamDaemon (Windows `dreamdaemon.exe`/`dd.exe`).
    Server,
}

impl Host {
    #[cfg(any(test, feature = "validate"))]
    pub(crate) const ALL: [Host; 2] = [Host::Compiler, Host::Server];

    /// Classify the executable by file name (case-insensitive; `.exe` and the installer's `_orig` stripped).
    fn from_exe_name(name: &str) -> Option<Host> {
        let name = name.to_ascii_lowercase();
        let stem = name.strip_suffix(".exe").unwrap_or(&name);
        let stem = stem.strip_suffix("_orig").unwrap_or(stem);
        match stem {
            "dm" | "dreammaker" => Some(Host::Compiler),
            "dd" | "dreamdaemon" => Some(Host::Server),
            _ => None,
        }
    }

    /// The host of this process, if it is one dmfast patches.
    pub(crate) fn current() -> Option<Host> {
        let exe = std::env::current_exe().ok()?;
        Host::from_exe_name(&exe.file_name()?.to_string_lossy())
    }
}

/// What a feature promises by its effect on behaviour: one that is both a speedup and a fix is a `Fix`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Output and behaviour identical to stock (identical `.dmb`, `DMFAST_VERIFY`). On by default.
    Perf,
    /// Changes behaviour in some case: opt-in, in a [`Tier`], with a test world `tests/fixes/<key>.dme`.
    Fix,
}

/// The programs a feature patches (a set).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hosts(u8);

impl Hosts {
    pub const COMPILER: Hosts = Hosts(1);
    pub const SERVER: Hosts = Hosts(2);
    pub const BOTH: Hosts = Hosts(3);

    pub(crate) fn has(self, host: Host) -> bool {
        let bit = match host {
            Host::Compiler => Self::COMPILER.0,
            Host::Server => Self::SERVER.0,
        };
        self.0 & bit != 0
    }
}

/// Fix tiers, as in BUGFIXES.md: `safe` only changes the buggy case; `extended` changes behaviour a working game
/// could rely on; `breaking` turns code stock accepts into errors or changes results. A tier includes lower ones.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Safe,
    Extended,
    Breaking,
}

impl Tier {
    pub(crate) const ALL: [Tier; 3] = [Tier::Safe, Tier::Extended, Tier::Breaking];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Tier::Safe => "safe",
            Tier::Extended => "extended",
            Tier::Breaking => "breaking",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Status {
    Stable,
    /// Installs only when selected by key or by `experimental` (a fix also needs its tier); never by default.
    Experimental,
    /// Never installs; the key stays valid (selecting it is no error). The reason shows under `DMFAST_STATS`.
    Deprecated(&'static str),
}

/// Oldest supported build (`version * 10000 + build`); a feature `fixed_in` at or below it fails validation.
#[cfg(any(test, feature = "validate"))]
pub(crate) const OLDEST_SUPPORTED: u32 = 515_1647;

/// The BYOND builds (`version * 10000 + build`) a feature is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Builds {
    /// First build it is offered on (0: any).
    pub(crate) since: u32,
    /// First build with BYOND's own fix (skipped from there on), and its bug report.
    pub(crate) fixed_in: Option<(u32, &'static str)>,
}

/// What an install function gets: the images it reads and writes, and the lookups features share
/// ([`Ctx::shared`]), only while features install ([`crate::image::Image`]).
pub struct Ctx {
    core: Image,
    /// `None`: the loader didn't report it (Linux) or its headers didn't parse (Windows).
    exe: Option<Image>,
    shared: Memo,
}

impl Ctx {
    pub(crate) fn new(core: Image, exe: Option<Image>) -> Ctx {
        Ctx { core, exe, shared: Memo::default() }
    }

    /// The core library (byondcore.dll, libbyond.so).
    pub fn core(&self) -> &Image {
        &self.core
    }

    /// The program; `Err` if the loader left it out of its list (Linux) or its headers didn't parse (Windows).
    pub fn exe(&self) -> Result<&Image> {
        self.exe.as_ref().ok_or_else(|| "program image not reported by the loader".into())
    }

    /// Another loaded module (e.g. `byondext.dll`, `libc.so.6`). Its code is copied when first opened, so its
    /// readers see it as shipped too (`crate::snapshot`).
    pub fn module(&self, name: &str) -> Result<Image> {
        Image::loaded(name).inspect(crate::snapshot::take)
    }

    /// `f(self)`, run once for `key` (a lookup several features start from) and kept until install ends. Each
    /// caller relies on what `f` relied on, as if it had run `f` itself, so another feature's write over those
    /// bytes conflicts even if the first caller didn't install; the callers themselves may write there (they share
    /// its sites). `f` only reads: what else it recorded (writes, actions) is the first caller's. Outside an install
    /// (e.g. in [`Feature::offered_if`]) `f` runs on every call.
    pub fn shared<T: 'static>(&self, key: &'static str, f: impl FnOnce(&Ctx) -> Result<T>) -> Result<&T> {
        let kept = self.shared.iter().find(|s| s.key == key && s.relied.is_some());
        let s = match kept {
            Some(s) => {
                crate::ledger::replay(s.relied.as_deref().unwrap_or_default());
                s
            }
            None => {
                let mark = crate::ledger::mark();
                let value: Box<dyn Any> = Box::new(f(self));
                let relied = mark.map(|m| crate::ledger::share(m, key));
                self.shared.push(Shared { key, value, relied })
            }
        };
        let value = s.value.downcast_ref::<Result<T>>();
        value
            .ok_or_else(|| Error::from(format!("shared lookup {key} kept as another type")))?
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// [`Ctx::shared`]'s lookups: an append-only list, so what it hands out stays put while the `Ctx` lives.
#[derive(Default)]
struct Memo {
    entry: OnceCell<Shared>,
    next: OnceCell<Box<Memo>>,
}

struct Shared {
    key: &'static str,
    /// The lookup's `Result<T>`.
    value: Box<dyn Any>,
    /// The ranges it relied on, replayed into each later caller's; `None`: it ran outside an install (not kept).
    relied: Option<Vec<crate::ledger::SharedRange>>,
}

impl Memo {
    fn iter(&self) -> impl Iterator<Item = &Shared> {
        std::iter::successors(Some(self), |m| m.next.get().map(|b| &**b)).filter_map(|m| m.entry.get())
    }

    /// `s`, added at the end.
    fn push(&self, s: Shared) -> &Shared {
        let mut m = self;
        while m.entry.get().is_some() {
            m = m.next.get_or_init(Box::default);
        }
        m.entry.get_or_init(|| s)
    }
}

impl Ctx {
    /// Run `f` once this feature's writes are done (not after `Err` or a conflict), still at install time, in
    /// order. Publish state other features read here, so a feature that doesn't install leaves none.
    pub fn on_commit(&self, f: impl FnOnce() + Send + 'static) {
        crate::ledger::on_commit(Box::new(f));
    }

    /// Run `f` if this feature (or the [`Parts::part`] it is called in) doesn't install, newest first.
    pub fn on_abort(&self, f: impl FnOnce() + Send + 'static) {
        crate::ledger::on_abort(Box::new(f));
    }

    /// Independently installed parts of this feature.
    pub fn parts(&self) -> Parts<'_> {
        Parts { _cx: PhantomData, done: Vec::new(), missing: Vec::new() }
    }
}

/// Parts of a feature that install independently: a part that fails is left out with everything it recorded
/// (writes, relied ranges, [`Ctx::on_commit`]; its [`Ctx::on_abort`] actions run), the others still apply.
pub struct Parts<'a> {
    _cx: PhantomData<&'a Ctx>,
    done: Vec<String>,
    missing: Vec<Error>,
}

impl Parts<'_> {
    /// Install a part: `Ok(detail)` lists `detail`, `Err(why)` undoes it and lists `why`. Whether it installed.
    pub fn part(&mut self, f: impl FnOnce() -> Result<String>) -> bool {
        let mark = crate::ledger::mark();
        match f() {
            Ok(detail) => {
                self.done.push(detail);
                true
            }
            Err(why) => {
                if let Some(m) = mark {
                    crate::ledger::rollback(m);
                }
                self.missing.push(why);
                false
            }
        }
    }

    /// [`Parts::part`] listed as `name`, or `name (why)`.
    pub fn named(&mut self, name: &str, f: impl FnOnce() -> Result<()>) -> bool {
        self.part(|| f().map(|()| name.to_string()).map_err(|e| format!("{name} ({e})").into()))
    }

    /// List a part left out without [`Parts::part`] (what was read to decide stays relied on).
    pub fn skip(&mut self, why: impl Into<Error>) {
        self.missing.push(why.into());
    }

    /// Whether some part installed.
    pub fn any(&self) -> bool {
        !self.done.is_empty()
    }

    /// `Err` only if no part installed, else the parts and why the others are missing.
    pub fn finish(self) -> Result<String> {
        let missing = self.missing.iter().map(Error::as_str).collect::<Vec<_>>().join("; ");
        if self.done.is_empty() {
            return Err(missing.into());
        }
        if self.missing.is_empty() {
            Ok(self.done.join(", "))
        } else {
            Ok(format!("{}; without: {missing}", self.done.join(", ")))
        }
    }
}

/// Locates, checks and writes a feature; returns its detail line or why it can't install. Its writes are held
/// until it returns (see [`crate::ledger`]): all applied after `Ok`, none after `Err`.
pub type InstallFn = fn(&Ctx) -> Result<String>;

/// [`Install::os`] bits.
pub const WINDOWS: u8 = 1;
pub const LINUX: u8 = 2;

/// Built by [`install!`](crate::feature::install): this OS's install fn (`None`: not here), OSes with one.
#[derive(Clone, Copy)]
pub struct Install {
    pub os: u8,
    pub run: Option<InstallFn>,
}

/// One speed patch or bug fix. Build with [`Feature::perf`] / [`Feature::fix`] and the modifiers below.
pub struct Feature {
    /// Stable public name: `fixes` selection, `DMFAST_DISABLE`, dependency guards ([`crate::deps`]), docs.
    pub key: &'static str,
    /// What it does, for the report lines.
    pub(crate) title: &'static str,
    pub kind: Kind,
    pub(crate) hosts: Hosts,
    /// Installs without being selected.
    pub(crate) on_by_default: bool,
    /// Fixes only.
    pub(crate) tier: Option<Tier>,
    pub(crate) status: Status,
    pub(crate) byond: Builds,
    /// Installs after these (if in the same list): data one sets up and the other reads, or a hook this one wraps.
    pub(crate) after: &'static [&'static Feature],
    /// Installs only if all of these applied (and after them).
    pub(crate) requires: &'static [&'static Feature],
    pub(crate) install: Install,
    /// Offered at all in this program (e.g. only if the code it changes exists); `None`: always.
    pub(crate) offered: Option<fn(&Ctx) -> bool>,
    /// Prints its counters at exit under `DMFAST_STATS`.
    pub(crate) stats: Option<fn()>,
    state: AtomicU8,
}

/// What the driver did with a feature in this process ([`Feature::state`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum State {
    /// Not for this OS or program, not offered, or never looked at.
    NotOffered = 0,
    /// Offered, not selected.
    Skipped,
    /// Selected, but `DMFAST_DISABLE` lists it.
    Disabled,
    /// BYOND has the fix itself (`fixed_in`), or this build predates `since`.
    OutOfRange,
    Deprecated,
    Applied,
    /// Tried and failed: a signature, a dependency guard, a required feature, or a write conflict.
    Failed,
}

impl Feature {
    const fn new(kind: Kind, hosts: Hosts, key: &'static str, title: &'static str, install: Install) -> Feature {
        Feature {
            key,
            title,
            kind,
            hosts,
            on_by_default: matches!(kind, Kind::Perf),
            tier: None,
            status: Status::Stable,
            byond: Builds { since: 0, fixed_in: None },
            after: &[],
            requires: &[],
            install,
            offered: None,
            stats: None,
            state: AtomicU8::new(State::NotOffered as u8),
        }
    }

    /// A speed patch: on unless disabled.
    pub const fn perf(hosts: Hosts, key: &'static str, title: &'static str, install: Install) -> Feature {
        Feature::new(Kind::Perf, hosts, key, title, install)
    }

    /// A bug fix in `tier`: off unless selected.
    pub const fn fix(hosts: Hosts, key: &'static str, title: &'static str, tier: Tier, install: Install) -> Feature {
        let mut f = Feature::new(Kind::Fix, hosts, key, title, install);
        f.tier = Some(tier);
        f
    }

    pub const fn after(mut self, features: &'static [&'static Feature]) -> Feature {
        self.after = features;
        self
    }

    pub const fn requires(mut self, features: &'static [&'static Feature]) -> Feature {
        self.requires = features;
        self
    }

    /// BYOND fixed the bug itself in `build` (`version * 10000 + build`); `report` is the bug report.
    pub const fn fixed_in(mut self, build: u32, report: &'static str) -> Feature {
        self.byond.fixed_in = Some((build, report));
        self
    }

    /// Offered from BYOND `build` on.
    pub const fn since(mut self, build: u32) -> Feature {
        self.byond.since = build;
        self
    }

    /// On without being selected (a fix that is right for everyone).
    pub const fn default_on(mut self) -> Feature {
        self.on_by_default = true;
        self
    }

    pub const fn experimental(mut self) -> Feature {
        self.status = Status::Experimental;
        self
    }

    pub const fn deprecated(mut self, reason: &'static str) -> Feature {
        self.status = Status::Deprecated(reason);
        self
    }

    pub const fn offered_if(mut self, f: fn(&Ctx) -> bool) -> Feature {
        self.offered = Some(f);
        self
    }

    pub const fn stats(mut self, f: fn()) -> Feature {
        self.stats = Some(f);
        self
    }

    fn state(&self) -> State {
        const ALL: [State; 7] = [
            State::NotOffered,
            State::Skipped,
            State::Disabled,
            State::OutOfRange,
            State::Deprecated,
            State::Applied,
            State::Failed,
        ];
        ALL.get(self.state.load(Relaxed) as usize).copied().unwrap_or(State::NotOffered)
    }

    pub(super) fn set_state(&self, s: State) {
        self.state.store(s as u8, Relaxed);
    }

    pub fn applied(&self) -> bool {
        self.state() == State::Applied
    }

    /// `fix ` for a fix, nothing for a speed patch: the report lines' prefix before the key.
    pub(crate) fn kind_prefix(&self) -> &'static str {
        match self.kind {
            Kind::Perf => "",
            Kind::Fix => "fix ",
        }
    }
}

/// An [`Install`] from each OS's install function, a safe [`InstallFn`]. An OS without an arm doesn't offer the
/// feature.
///
/// ```ignore
/// install!(windows: windows::install, linux: linux::install)
/// ```
#[macro_export]
macro_rules! install {
    (windows: $w:expr, linux: $l:expr $(,)?) => {
        $crate::feature::Install {
            os: $crate::feature::WINDOWS | $crate::feature::LINUX,
            run: cfg_select! {
                windows => { Some($w as $crate::feature::InstallFn) }
                target_os = "linux" => { Some($l as $crate::feature::InstallFn) }
                _ => { None }
            },
        }
    };
    (windows: $w:expr $(,)?) => {
        $crate::feature::Install {
            os: $crate::feature::WINDOWS,
            run: cfg_select! {
                windows => { Some($w as $crate::feature::InstallFn) }
                _ => { None }
            },
        }
    };
    (linux: $l:expr $(,)?) => {
        $crate::feature::Install {
            os: $crate::feature::LINUX,
            run: cfg_select! {
                target_os = "linux" => { Some($l as $crate::feature::InstallFn) }
                _ => { None }
            },
        }
    };
}
pub use crate::install;
