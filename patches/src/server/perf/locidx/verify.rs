//! `DMFAST_VERIFY=1`: every lookup the set answers also runs the stock scan, whose answer is used; a different
//! answer is a mismatch.

use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// With verify on, the stock scan's answer, counting a `hit` that differs; else `hit`.
#[inline(always)]
pub(super) fn answer(hit: bool, stock: impl FnOnce() -> bool) -> bool {
    if !verify() {
        return hit;
    }
    let s = stock();
    if s != hit {
        MISMATCHES.bump();
    }
    s
}
