//! `poll`: the compiler's progress thread polls every 10 ms instead of 250, as the compiler waits for it on exit.
//!
//! Dependency guard: none, it only changes the operand its signature matches.

use crate::base::feature::{Ctx, Feature, Hosts, install};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
use windows as os;

pub static POLL: Feature =
    Feature::perf(Hosts::COMPILER, "poll", "progress poll", install!(windows: windows::install, linux: linux::install))
        .offered_if(has_progress_thread);

fn has_progress_thread(c: &Ctx) -> bool {
    c.exe().is_ok_and(os::has_progress_thread)
}
