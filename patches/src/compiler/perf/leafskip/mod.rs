//! `leafskip`: the compile-pass leaf skip. Each pass visits every node through `pass(node, mode)`, whose handlers only
//! act on nodes with the finished flag clear, so a finished node with no children returns at once (no caller uses
//! the result). The flag and child count come from the pass function's own checks.
//!
//! `deps` guards the pass function (every handler in it ignores finished leaves) and its callers.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod verify;
#[cfg(windows)]
mod windows;

pub static LEAFSKIP: Feature = Feature::perf(
    Hosts::COMPILER,
    "leafskip",
    "compile-pass leaf skip",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    verify::print_stats();
}
