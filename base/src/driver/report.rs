//! `DMFAST_STATS`: the summary on the stats line and each feature's counters at exit. `DMFAST_VERIFY`: features
//! check their fast paths against the original.

use std::fmt;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use crate::driver::select::has_selection;
use crate::driver::{Config, INSTALLED};
use crate::feature::{Host, Kind};

/// `DMFAST_STATS` is set (read once).
pub fn stats() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("DMFAST_STATS").is_some());
    *ON
}

static VERIFY: AtomicBool = AtomicBool::new(false);

/// `DMFAST_VERIFY` is set. One plain load, so hooks may test it on every call.
#[inline(always)]
pub fn verify() -> bool {
    VERIFY.load(Relaxed)
}

/// Read `DMFAST_VERIFY`, before any feature installs.
pub(crate) fn read_verify() {
    VERIFY.store(std::env::var_os("DMFAST_VERIFY").is_some(), Relaxed);
}

/// An event counter for a feature's stats (`static HITS: Counter = Counter::new();`, `HITS.bump()`, `{HITS}` in its
/// stats line). Relaxed: counts may lag a little across threads.
#[derive(Default)]
pub struct Counter(AtomicU32);

impl Counter {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    pub fn bump(&self) {
        self.0.fetch_add(1, Relaxed);
    }

    /// [`Counter::bump`], and whether this was one of the first `n` (to print the first few mismatches).
    pub fn bump_first(&self, n: u32) -> bool {
        self.0.fetch_add(1, Relaxed) < n
    }

    pub fn get(&self) -> u32 {
        self.0.load(Relaxed)
    }
}

impl fmt::Display for Counter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

/// `mimalloc <x.y.z>, <split>` for the stats line, from `mi_version()` (`30504` is 3.5.4).
pub(crate) fn mimalloc(version: i32, split: &str) -> String {
    format!("mimalloc {}.{}.{}, {split}", version / 10000, version / 100 % 100, version % 100)
}

/// `applied=<perf keys>; fixes=<fix keys>` in install order (`none` if empty, `fixes=unselected` without a selection).
pub(super) fn summary(config: &Config, host: Host) -> String {
    let keys = |kind: Kind| {
        let keys: Vec<&str> =
            config.ordered(host).into_iter().filter(|f| f.kind == kind && f.applied()).map(|f| f.key).collect();
        if keys.is_empty() { "none".to_string() } else { keys.join(",") }
    };
    let fixes = if has_selection() { keys(Kind::Fix) } else { "unselected".to_string() };
    format!("applied={}; fixes={fixes}", keys(Kind::Perf))
}

/// With `DMFAST_STATS`: every feature's counters, after a `dmfast: bug:` line if an image was used after install.
pub fn print_stats() {
    if !stats() {
        return;
    }
    if crate::image::used_late() {
        eprintln!("dmfast: bug: install-time Image used after install");
    }
    let Some(&(config, host)) = INSTALLED.get() else { return };
    for f in config.ordered(host) {
        if let Some(stats) = f.stats
            && f.applied()
        {
            stats();
        }
    }
}

/// [`print_stats`] as an `atexit` handler (Linux; on Windows the DLL's detach prints them).
#[cfg(target_os = "linux")]
pub(super) extern "C" fn print_stats_at_exit() {
    print_stats();
}
