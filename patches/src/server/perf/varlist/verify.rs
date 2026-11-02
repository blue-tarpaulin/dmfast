//! `DMFAST_VERIFY=1`: every shortcut also runs the merge; a different list id is a mismatch.

use crate::base::driver::Counter;

pub(super) static MISMATCHES: Counter = Counter::new();

/// Compare the shortcut's list id, `mine`, with the merge's.
pub(super) fn check(mine: u32, byond: u32) {
    // The first few are printed.
    if mine != byond && MISMATCHES.bump_first(10) {
        eprintln!("dmfast: varlist mismatch: ours {mine}, byond {byond}");
    }
}
