//! `strings`: the string table's free-slot search. Each new string scans the table from slot 0 for the first free
//! slot (0xFFFF stays reserved), and a full table grows by 10 entries. Instead a hint H, with every slot below H in
//! use, is where the scan starts; the `.dmb` save lowers H when it frees a slot, and a full table grows to
//! max(count*2, count+10).
//!
//! It's safe because the save's store is the only one that frees a slot, and a table with no slots (new, or emptied
//! by the global reset, which zeroes the count) restarts H at 0: the scan finds the slot BYOND's would, so ids and the
//! `.dmb` stay the same. `deps` guards the insert, the save, and the reset.

use std::sync::atomic::AtomicU32;

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static STRINGS: Feature = Feature::perf(
    Hosts::COMPILER,
    "strings",
    "string table",
    install!(windows: windows::install, linux: linux::install),
)
.stats(verify::print_stats);

/// H. Only the stubs read and write it, on whichever thread interns.
static HINT: AtomicU32 = AtomicU32::new(0);

/// The reserved slot the scan skips.
const RESERVED: u32 = 0xFFFF;
