//! `DMFAST_VERIFY=1`: every hit also runs the scan, whose answer is used and stored; a different answer is a
//! mismatch.

use super::scan;
use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// With verify on, replace a hit's index `idx` with the scan's, counting a difference.
///
/// # Safety
/// As [`scan`].
#[inline(always)]
pub(super) unsafe fn recheck(idx: &mut u16, children: usize, count: u16) {
    if !verify() {
        return;
    }
    // SAFETY: live children (caller).
    let s = unsafe { scan(children, count) };
    if s != *idx {
        MISMATCHES.bump();
        *idx = s;
    }
}
