//! `DMFAST_VERIFY=1`: every remembered answer is compared with a walk up the parents' tables.

use std::ptr;

use super::Entry;
use crate::base::driver::Counter;

pub(super) static MISMATCHES: Counter = Counter::new();

/// A remembered answer against the walk's: the same entry, or none for both.
pub(super) fn check(remembered: Option<&Entry>, walked: Option<&Entry>) {
    if remembered.map(ptr::from_ref) != walked.map(ptr::from_ref) {
        MISMATCHES.bump();
    }
}
