//! `DMFAST_VERIFY=1`: every compare answered from the tables also runs BYOND's; a different result is a mismatch.

use super::windows::{Found, original};
use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// The result to return for `a` and `b`: `ours`, or with verify on, BYOND's (counting a mismatch if it differs).
///
/// # Safety
/// `a` and `b` are NUL-terminated strings.
#[inline(always)]
pub(super) unsafe fn check(f: &Found, a: *const u8, b: *const u8, ours: i32) -> i32 {
    if !verify() {
        return ours;
    }
    // SAFETY: NUL-terminated (caller).
    let byond = unsafe { original(f, a, b) };
    if byond != ours {
        MISMATCHES.bump();
    }
    byond
}
