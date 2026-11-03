//! `DMFAST_VERIFY=1`: every index hit also runs the original, whose id is used and recorded; a different id is a
//! mismatch.

use super::Index;
use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// With verify on, hold back the hit `id` for (hash) `h` so the original runs: true.
#[inline(always)]
pub(super) fn hold(ix: &mut Index, h: u32, id: u32) -> bool {
    if !verify() {
        return false;
    }
    ix.held = Some((h, id));
    true
}

/// The original returned `id` for (hash) `h`: a mismatch if it differs from a hit held back for it.
#[inline(always)]
pub(super) fn check(ix: &mut Index, h: u32, id: u32) {
    if ix.held.take().is_some_and(|(held, hit)| held == h && hit != id) {
        MISMATCHES.bump();
    }
}
