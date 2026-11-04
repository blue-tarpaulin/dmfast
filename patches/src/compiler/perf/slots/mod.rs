//! `slots`: the compiler's other id tables' free-slot search. Each allocator scans its table from slot 0 for the first
//! free (null) slot, skipping the reserved 0xFFFF, and fills it. Instead each table keeps a hint H, with every slot
//! below H in use, where the scan starts.
//!
//! It's safe because slots are only filled until the global reset frees them all, filling slots elsewhere only makes H
//! conservative, and each H is checked before use (within the count, the slot below it in use; else the scan starts at
//! 0). Windows resets H with the global reset; Linux lowers it where a setter can empty a slot. `DMFAST_VERIFY=1` also
//! scans from slot 0 and counts disagreements. `deps` guards the reset.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::print_stats;
#[cfg(windows)]
use windows::print_stats;

pub static SLOTS: Feature =
    Feature::perf(Hosts::COMPILER, "slots", "slot hints", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);
