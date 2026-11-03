//! `DMFAST_VERIFY=1`: every answer is compared with the original scan; on a mismatch the scan runs instead.

use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// The original scan: the n-th (0-based) child in `kids` named `name`, or 0.
pub(super) fn scan(kids: &[u32], name_of: impl Fn(usize) -> u32, name: u32, n: u16) -> usize {
    kids.iter().filter(|&&c| name_of(c as usize) == name).nth(n as usize).map_or(0, |&c| c as usize)
}

/// Whether the answer `r` may be used: verify is off, or `stock` gives the same (else a mismatch).
#[inline(always)]
pub(super) fn agrees(r: usize, stock: impl FnOnce() -> usize) -> bool {
    if !verify() || stock() == r {
        return true;
    }
    MISMATCHES.bump();
    false
}
