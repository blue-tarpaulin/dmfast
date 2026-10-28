//! `dbclose` (forum report 2983292): closing or reopening a `/database` that ran queries frees its query list once.
//!
//! `Close()` frees the query array (pointer +18h, count +1Ch, capacity +20h) but keeps the pointer and capacity, so
//! the next close frees it again. The fix clears both after the free. `Open()` also sets the new name before closing
//! the old file, so the old file's shared connection (keyed by name) is left open; the fix closes first. BYOND made
//! the same two changes in 516.1670.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static DBCLOSE: Feature = Feature::fix(
    Hosts::SERVER,
    "dbclose",
    "closing or reopening a database frees its queries once",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
)
.fixed_in(516_1670, "https://www.byond.com/forum/post/2983292");

/// Offsets of the query array pointer and capacity in a `/database`.
const ARRAY: u8 = 0x18;
const CAPACITY: u8 = 0x20;
