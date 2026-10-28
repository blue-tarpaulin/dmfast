//! `fastexit`: the compiler exits once the `.dmb` is written and the summary printed, skipping its teardown.
//!
//! Dependency guard (`deps`): the core library code it skips or calls: the builder destructor (Linux), the resource
//! lock release that must unlink each entry, and its lock list's users (Windows).

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static FASTEXIT: Feature =
    Feature::perf(Hosts::COMPILER, "fastexit", "fast exit", install!(windows: windows::install, linux: linux::install))
        .offered_if(super::found);
