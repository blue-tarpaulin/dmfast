//! `DMFAST_VERIFY=1`: every CRC also runs the original, whose result is used; a different result is a mismatch.

use super::FOUND;
use crate::base::driver::{Counter, verify};

static CHECKED: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// The original's CRC, counting `ours` if it differs.
///
/// # Safety
/// `data..data+len` is readable.
pub(super) unsafe fn original(ours: u32, seed: u32, data: *const u8, len: u32) -> u32 {
    // SAFETY: the original is BYOND's CRC (install), on readable data (caller).
    let theirs = unsafe { FOUND.get().original.get()(seed, data, len) };
    if ours != theirs {
        MISMATCHES.bump();
    }
    CHECKED.bump();
    theirs
}

pub(super) fn print_stats() {
    if verify() && FOUND.is_set() {
        eprintln!("dmfast: CRC-32 verify checked={CHECKED} mismatches={MISMATCHES}");
    }
}
