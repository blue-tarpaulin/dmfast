//! `fscache`: the compiler's file system queries are answered from a cache (Windows).
//!
//! byondcore's `FindFirstFileW` (with `FindClose`), `GetFileAttributesW`, `GetFileAttributesExW` (standard level), and
//! `_wstat64i32` are cached by exact path, including the error they leave. A miss of `FindFirstFileW` or `_wstat64i32`
//! is answered from a listing of its directory where possible. Every call from dm.exe, byondcore, or byondext that can
//! modify the file system clears the cache, and the paths it touches are never cached again; changes by other
//! processes aren't seen. Install refuses a module that imports a modifying call the cache doesn't watch.
//! `DMFAST_VERIFY=1` also makes the real call for each answer from a listing and compares.
//!
//! No dependency guard: it only replaces imports.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod windows;

pub static FSCACHE: Feature =
    Feature::perf(Hosts::COMPILER, "fscache", "file cache", install!(windows: windows::install)).stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    windows::print_stats();
}
