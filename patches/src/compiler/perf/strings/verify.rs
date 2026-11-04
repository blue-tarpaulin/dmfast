//! `DMFAST_VERIFY=1`: before each scan, BYOND's scan from slot 0 must not find a free slot below the hint.

use super::RESERVED;
use crate::base::code::live_u32;
use crate::base::driver::{Counter, verify};

static CALLS: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// Called by the verify stubs with the table, its count, and the hint the scan is about to start from.
///
/// # Safety
/// `table` holds `count` slots (the scan's own registers).
pub(super) unsafe extern "C" fn check(table: usize, count: u32, hint: u32) {
    let start = if hint > count { 0 } else { hint };
    // SAFETY: below `count` (caller).
    let free = |i: u32| i != RESERVED && unsafe { live_u32(table + 4 * i as usize) } == 0;
    let first = (0..count).find(|&i| free(i)).unwrap_or(count);
    CALLS.bump();
    if first < start {
        MISMATCHES.bump();
    }
}

pub(super) fn print_stats() {
    if verify() {
        eprintln!("dmfast: string table hint calls={CALLS} mismatches={MISMATCHES}");
    }
}
