//! `DMFAST_VERIFY=1`: every skip still runs the child loops, and a key one of the scope's children holds in the probed
//! table is a mismatch.

use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// Whether a skip is held back (verify is on), counting a mismatch if `has_key` finds the key.
#[inline(always)]
pub(super) fn holds(has_key: impl FnOnce() -> bool) -> bool {
    if !verify() {
        return false;
    }
    if has_key() {
        MISMATCHES.bump();
    }
    true
}
