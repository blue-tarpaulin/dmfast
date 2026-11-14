//! `DMFAST_VERIFY=1`: an array about to be kept has each of its messages back on its pool's free list ([`all_free`]).

use super::{HEAD, NEXT, POOL, SIZE, SLOTS, TAKEN};
use crate::base::code::read;
use crate::base::driver::Counter;

pub(super) static MISMATCHES: Counter = Counter::new();

/// Whether the free list of `pool` holds each message of its `array` once, none of them taken.
///
/// # Safety
/// `pool` is a live pool and `array` its array.
pub(super) unsafe fn all_free(pool: usize, array: usize) -> bool {
    let mut seen = [false; SLOTS];
    // SAFETY: a live pool (caller); indices are checked before use.
    let mut i = unsafe { read::<i32>(pool + HEAD) };
    let mut n = 0;
    while i >= 0 {
        let k = i as usize;
        if k >= SLOTS || seen[k] {
            return mismatch(format_args!("free list index {i} after {n}"));
        }
        seen[k] = true;
        n += 1;
        let m = array + k * SIZE;
        // SAFETY: message k of the array.
        unsafe {
            if read::<u8>(m + TAKEN) != 0 || read::<usize>(m + POOL) != pool {
                return mismatch(format_args!("message {k} taken or of another pool"));
            }
            i = read(m + NEXT);
        }
    }
    n == SLOTS || mismatch(format_args!("{n} of {SLOTS} messages free"))
}

fn mismatch(what: std::fmt::Arguments) -> bool {
    if MISMATCHES.bump_first(5) {
        eprintln!("dmfast: msgpool mismatch: {what}");
    }
    false
}
