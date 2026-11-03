//! `DMFAST_VERIFY=1`: every answer is compared with the original walk; on a mismatch the walk runs instead.

use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// Whether the answer `r` may be used: verify is off, or `stock` gives the same (else a mismatch).
#[inline(always)]
pub(super) fn agrees(r: usize, stock: impl FnOnce() -> usize) -> bool {
    if !verify() || stock() == r {
        return true;
    }
    MISMATCHES.bump();
    false
}
