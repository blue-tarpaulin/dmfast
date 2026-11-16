//! A feature is one speed patch or bug fix: a `static Feature` holding its key, kind, hosts, lifecycle, ordering
//! and per-OS install function, listed in [`crate::driver::Config`]. [`crate::driver`] selects,
//! orders and installs them; nothing else lists features.
//!
//! ```ignore
//! pub(crate) static CRC: Feature =
//!     Feature::perf(Hosts::SERVER, "crc", "CRC-32", install!(windows: windows::install, linux: linux::install))
//!         .stats(print_stats);
//! ```

mod ctx;

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

pub use ctx::{Ctx, Parts};

use crate::Result;
use crate::driver::Counts;

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

    /// Classify the executable by file name (case-insensitive, `.exe` stripped).
    fn from_exe_name(name: &str) -> Option<Host> {
        let name = name.to_ascii_lowercase();
        let stem = name.strip_suffix(".exe").unwrap_or(&name);
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

pub use dmfast_format::selection::Tier;

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

/// Locates, checks and writes a feature; returns its detail line or why it can't install. Its writes are held
/// until it returns (see [`crate::ledger`]): all applied after `Ok`, none after `Err`.
#[doc(hidden)]
pub type InstallFn = fn(&Ctx) -> Result<String>;

/// Built by [`install!`](crate::feature::install): this OS's install fn (`None`: not here).
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct Install(pub Option<InstallFn>);

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
    /// Called with the program's counts once every feature is installed.
    pub(crate) finish: Option<fn(Counts)>,
    applied: AtomicBool,
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
            finish: None,
            applied: AtomicBool::new(false),
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

    /// Install after these, if listed: data one sets up and this one reads, or another's hook this one wraps. Only
    /// these may have writes this feature's `wrap_code` and `wrap_data` write over.
    pub const fn after(mut self, features: &'static [&'static Feature]) -> Feature {
        self.after = features;
        self
    }

    /// Install only if all of these applied (and after them). To wrap one's writes, list it in `after` too.
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

    /// Installs only when selected by key or by `experimental` (a fix also needs its tier); never by default.
    pub const fn experimental(mut self) -> Feature {
        self.status = Status::Experimental;
        self
    }

    /// Never installs; the key stays valid. `reason` shows under `DMFAST_STATS`.
    pub const fn deprecated(mut self, reason: &'static str) -> Feature {
        self.status = Status::Deprecated(reason);
        self
    }

    /// Offered in this program only if `f` says so (e.g. only if the code it changes exists).
    pub const fn offered_if(mut self, f: fn(&Ctx) -> bool) -> Feature {
        self.offered = Some(f);
        self
    }

    /// Print its counters at exit under `DMFAST_STATS` (and verify mismatches under `DMFAST_VERIFY`). Called only if
    /// it applied, so `f` needn't check that.
    pub const fn stats(mut self, f: fn()) -> Feature {
        self.stats = Some(f);
        self
    }

    /// Called once every feature is installed, still at install time, with the program's counts (this feature's
    /// included). Called only if it applied.
    pub const fn finish(mut self, f: fn(Counts)) -> Feature {
        self.finish = Some(f);
        self
    }

    pub(crate) fn mark_applied(&self) {
        self.applied.store(true, Relaxed);
    }

    pub fn applied(&self) -> bool {
        self.applied.load(Relaxed)
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
        $crate::feature::Install(cfg_select! {
            windows => { Some($w as $crate::feature::InstallFn) }
            target_os = "linux" => { Some($l as $crate::feature::InstallFn) }
            _ => { None }
        })
    };
    (windows: $w:expr $(,)?) => {
        $crate::feature::Install(cfg_select! {
            windows => { Some($w as $crate::feature::InstallFn) }
            _ => { None }
        })
    };
    (linux: $l:expr $(,)?) => {
        $crate::feature::Install(cfg_select! {
            target_os = "linux" => { Some($l as $crate::feature::InstallFn) }
            _ => { None }
        })
    };
}
pub use crate::install;
