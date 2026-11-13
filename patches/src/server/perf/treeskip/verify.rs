//! `DMFAST_VERIFY=1`: a flag walk [`super::walk`] starts late or skips checks the records it passes over; one with its
//! flag (or a start not in the list) is a mismatch.

use super::NEXT;
use crate::base::code::read;
use crate::base::driver::Counter;
use crate::server::layout::RECORD_FLAGS;

pub(super) static MISMATCHES: Counter = Counter::new();

/// `start` if no record before it (all of them when it's 0) carries `flag`, else `first` (a mismatch).
///
/// # Safety
/// `first` heads a live record list only this thread uses.
pub(super) unsafe fn checked(first: usize, start: usize, flag: u16) -> usize {
    let mut node = first;
    while node != start {
        if node == 0 {
            mismatch(format_args!("flag {flag:#x}: start {start:#x} not in the list"));
            return first;
        }
        // SAFETY: a live record (caller).
        let flags = unsafe { read::<u16>(node + RECORD_FLAGS) };
        if flags & flag != 0 {
            mismatch(format_args!("flag {flag:#x} on a record (flags {flags:#x}) before the start"));
            return first;
        }
        // SAFETY: as above.
        node = unsafe { read(node + NEXT) };
    }
    start
}

fn mismatch(what: std::fmt::Arguments) {
    if MISMATCHES.bump_first(5) {
        eprintln!("dmfast: treeskip mismatch: {what}");
    }
}
