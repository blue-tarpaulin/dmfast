//! `iconstates`: `icon_states()` without its quadratic duplicate check.
//!
//! `icon_states()` adds each state with `L[name] = null`; a key the association tree misses goes to the new-key
//! path, which scans every item first. In the builder's list that scan can't match: every item went in through the
//! same set, so the tree knows them all. [`set_state`] marks the list, and a platform stub sends a marked list's scan
//! straight to not-found. `DMFAST_VERIFY=1` runs every scan and checks it finds nothing.
//!
//! Dependency guard (`deps`): the builder, which must add to its list only through the set.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::Foreign;
use crate::base::driver::{Counter, verify};
use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static ICONSTATES: Feature = Feature::perf(
    Hosts::SERVER,
    "iconstates",
    "icon_states list",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// The associative set `(list, key, value)`, each DM value as two dwords (type, then data).
type SetFn = unsafe extern "C" fn(u32, u32, u32, u32, u32, u32);

/// Type tag of the builder's list (a plain `/list`).
const LIST: u8 = 0x0F;

/// No list is being built.
const NONE: u32 = u32::MAX;

/// The id of the list `icon_states()` is filling, or [`NONE`]; the stub compares it with the new-key path's list.
static BUILDING: AtomicU32 = AtomicU32::new(NONE);

static SET: Foreign<SetFn> = Foreign::new();
static STATES: Counter = Counter::new();

/// Replaces the builder's calls of the set: the same call, with the list marked as being built.
///
/// # Safety
/// Only as those calls, after `SET` is set.
unsafe extern "C" fn set_state(list_type: u32, list: u32, key_type: u32, key: u32, value_type: u32, value: u32) {
    let marked = if list_type as u8 == LIST { list } else { NONE };
    let previous = BUILDING.swap(marked, Relaxed);
    STATES.bump();
    // SAFETY: BYOND's set (found at install) with the builder's own arguments (caller).
    unsafe { SET.get()(list_type, list, key_type, key, value_type, value) };
    BUILDING.store(previous, Relaxed);
}

fn print_stats() {
    if SET.addr() != 0 {
        eprintln!(
            "dmfast: icon_states states={STATES} verify={} checked={} mismatches={}",
            verify(),
            verify::CHECKED,
            verify::MISMATCHES
        );
    }
}
