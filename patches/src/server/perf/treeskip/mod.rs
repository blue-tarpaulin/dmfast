//! `treeskip`: SendMaps' send of a client's record changes starts each walk of the record list at the first record
//! the walk can find, and skips the walks that would find nothing.
//!
//! The send walks the client's whole record list (in id order) once per kind of change: once for the records not seen
//! this scan (they're removed), once for flag 2, and once for each of the flags 1, 4, 8, 40h, 80h, 100h, 400h, and
//! 800h; most walks find nothing. [`scan`], at the first walk, notes in one pass the first unseen record and the first
//! seen record with each flag; each walk then starts there, or on an empty list when there is none. The records before
//! that start have nothing the walk looks for, and a walk on an empty list is handled exactly as one that finds nothing
//! (the same count of 0 is written, and nothing else).
//!
//! It's safe because nothing between the scan and a walk sets a flag but the flag-2 walk, which sets 4 on what it
//! finds (the scan counts a record with 2 as having 4). The rest only clears flags and removes unseen records, which
//! leaves the others in place, and the unseen records are all removed before the flag walks (or the send stops for the
//! tick when there are 250 of them), so their flags aren't noted. `DMFAST_VERIFY=1`: each flag walk also checks the
//! records it skips; one with its flag is a mismatch (then the walk starts at the list's start).

use std::cell::Cell;

use crate::base::code::read;
use crate::base::driver::{Counter, verify};
use crate::base::feature::{Feature, Hosts, install};
use crate::server::layout::{RECORD_FLAGS, SEEN};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static TREESKIP: Feature = Feature::perf(
    Hosts::SERVER,
    "treeskip",
    "SendMaps change walks",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// Record tree: the first record in id order. Record: the next in id order.
const FIRST: usize = 0x04;
const NEXT: usize = 0x0C;
/// The flag whose walk sets [`PLACED_SETS`] on the records it finds.
const PLACED: u16 = 0x02;
const PLACED_SETS: u16 = 0x04;

static WALKED: Counter = Counter::new();
static SKIPPED: Counter = Counter::new();

thread_local! {
    /// The flags some seen record of the list this thread is sending carries, from [`scan`].
    static FOUND: Cell<u16> = const { Cell::new(0) };
    /// The first seen record with each flag in [`FOUND`] (by bit).
    static FIRSTS: [Cell<usize>; 16] = const { [const { Cell::new(0) }; 16] };
}

/// `start`, counted as a walk or, when none, as a skipped one.
fn counted(start: usize) -> usize {
    if start != 0 {
        WALKED.bump()
    } else {
        SKIPPED.bump()
    }
    start
}

/// The unseen-record walk's start, from the list's first record: notes the seen records' flags, and returns the first
/// unseen record (none: 0).
///
/// # Safety
/// `first` heads a live record list only this thread uses.
unsafe extern "C" fn scan(first: usize) -> usize {
    let (mut found, mut unseen) = (0u16, 0);
    FIRSTS.with(|firsts| {
        let mut node = first;
        while node != 0 {
            // SAFETY: a live record (caller).
            let mut flags = unsafe { read::<u16>(node + RECORD_FLAGS) };
            if flags & SEEN == 0 {
                if unseen == 0 {
                    unseen = node;
                }
            } else {
                if flags & PLACED != 0 {
                    flags |= PLACED_SETS;
                }
                let mut new = flags & !found;
                found |= new;
                while new != 0 {
                    firsts[new.trailing_zeros() as usize].set(node);
                    new &= new - 1;
                }
            }
            // SAFETY: as above.
            node = unsafe { read(node + NEXT) };
        }
    });
    FOUND.set(found);
    counted(unseen)
}

/// `flag`'s walk's start, from the list's first record: the first record that may carry the flag (none: 0).
///
/// # Safety
/// `first` heads the live record list [`scan`] last ran on, only this thread uses.
unsafe extern "C" fn walk(first: usize, flag: u32) -> usize {
    let flag = flag as u16;
    let start = if FOUND.get() & flag != 0 { FIRSTS.with(|f| f[flag.trailing_zeros() as usize].get()) } else { 0 };
    // SAFETY: a live list (caller).
    counted(if verify() { unsafe { verify::checked(first, start, flag) } } else { start })
}

fn print_stats() {
    eprint!("dmfast: treeskip walked={WALKED} skipped={SKIPPED}");
    if verify() {
        eprint!(" mismatches={}", verify::MISMATCHES);
    }
    eprintln!();
}
