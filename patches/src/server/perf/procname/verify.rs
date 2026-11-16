//! `DMFAST_VERIFY=1`: every lookup also runs BYOND's, and disagreements are counted.

use super::{COUNT, ENTRIES, LookupFn, search};
use crate::base::code::{Foreign, live_u32, read};
use crate::base::driver::{Counter, verify};

pub(super) static ORIGINAL: Foreign<LookupFn> = Foreign::new();
static CHECKED: Counter = Counter::new();
static HITS: Counter = Counter::new();
static ABSENT: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// Checks `got` (from a remembered entry if `hit`) against BYOND's lookup, and counts names the array doesn't have
/// (never remembered).
///
/// # Safety
/// As [`super::lookup`].
pub(super) unsafe fn check(map: usize, name: u32, depth: u32, flag: u32, got: u32, hit: bool) {
    CHECKED.bump();
    if hit {
        HITS.bump();
    }
    // SAFETY: BYOND's lookup with its caller's arguments, and the live map's entries.
    unsafe {
        if search(live_u32(map + ENTRIES) as usize, u32::from(read::<u16>(map + COUNT)), name).is_none() {
            ABSENT.bump();
        }
        if ORIGINAL.get()(map, name, depth, flag) != got {
            MISMATCHES.bump();
        }
    }
}

pub(super) fn print_stats() {
    if verify() {
        eprintln!(
            "dmfast: proc name lookup checked={CHECKED} remembered={HITS} absent={ABSENT} mismatches={MISMATCHES}"
        );
    }
}
