//! `DMFAST_VERIFY=1`: every answer the index gives is checked against the original scan; on a disagreement BYOND's
//! find-or-add answers and the index is rebuilt.

use super::{Key, entry, key_of};
use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// With verify on, whether the original scan (the first entry equal to `k`) differs from `found`, counted.
///
/// # Safety
/// `array` holds `count` live records.
pub(super) unsafe fn disagrees(array: u32, count: u32, k: Key, found: Option<u32>) -> bool {
    if !verify() {
        return false;
    }
    // SAFETY: `i < count` live records (caller).
    let wrong = (0..count).find(|&i| unsafe { key_of(entry(array, i)) } == k) != found;
    if wrong {
        MISMATCHES.bump();
    }
    wrong
}
