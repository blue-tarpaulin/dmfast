//! `DMFAST_VERIFY=1`: every shortcut also runs the merge; a different list id is a mismatch.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub(super) static MISMATCHES: AtomicU32 = AtomicU32::new(0);

/// Compare the shortcut's list id, `mine`, with the merge's.
pub(super) fn check(mine: u32, byond: u32) {
    // The first few are printed.
    if mine != byond && MISMATCHES.fetch_add(1, Relaxed) < 10 {
        eprintln!("dmfast: varlist mismatch: ours {mine}, byond {byond}");
    }
}
