//! `DMFAST_STATS`: the summary on the stats line and each feature's counters at exit. `DMFAST_VERIFY`: features
//! check their fast paths against the original.

use std::sync::LazyLock;

use crate::driver::select::has_selection;
use crate::driver::{Config, INSTALLED};
use crate::feature::{Host, Kind};

/// `DMFAST_STATS` is set (read once).
pub fn stats() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("DMFAST_STATS").is_some());
    *ON
}

/// `DMFAST_VERIFY` is set (read once).
pub fn verify() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("DMFAST_VERIFY").is_some());
    *ON
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
        if let (Some(stats), Some(_)) = (f.stats, f.install.run) {
            stats();
        }
    }
}

/// [`print_stats`] as an `atexit` handler (Linux; on Windows the DLL's detach prints them).
#[cfg(target_os = "linux")]
pub(super) extern "C" fn print_stats_at_exit() {
    print_stats();
}
