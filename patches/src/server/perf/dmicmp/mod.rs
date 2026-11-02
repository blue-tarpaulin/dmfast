//! `dmicmp`: icon state names are compared with two lookup tables instead of two locale calls per character
//! (Windows).
//!
//! BYOND's compare skips characters that aren't letters or digits and ignores case: for each character it calls the
//! C library's `toupper` and `isalnum`, each a per-thread locale lookup. The tables hold those calls' answers for all
//! 256 bytes, built once from the same functions. They're used only while the thread's ctype table is the C locale's
//! (static, inside ucrtbase); any other locale uses BYOND's compare. `DMFAST_VERIFY=1` compares every answer with
//! BYOND's.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod verify;
#[cfg(windows)]
mod windows;

pub static DMICMP: Feature =
    Feature::perf(Hosts::SERVER, "dmicmp", "icon state name compare", install!(windows: windows::install))
        .stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    windows::print_stats();
}
