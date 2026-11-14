//! `msgpool`: SendMaps' message pools are reused instead of being built and freed every tick.
//!
//! A map thread doesn't send its messages itself: it copies each into a slot of a message pool (256 messages of
//! 43Ch bytes, ~270 KiB), and the main thread sends them once the threads are done. Every tick each thread starts
//! without a pool, so each thread that sends anything allocates a pool and constructs its 256 messages, and the main
//! thread destroys and frees it after the last one is sent. Here the destroy of such a pool keeps its message array
//! (up to [`KEPT`]) and frees only the pool, and the next pool built takes a kept array and rebuilds it in place: each
//! message destroyed and constructed again, and the free list linked in order, as a new array would be. It's safe
//! because a pool is destroyed only when nothing refers to it (every message taken from it came back), a kept array
//! belongs to no pool until one takes it, and the messages hold the same state a new array's would. `DMFAST_VERIFY=1`
//! checks, when an array is kept, that each of its messages is back on the pool's free list; if not (a mismatch), it's
//! freed as stock.

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::base::code::{read, write};
use crate::base::driver::{Counter, verify};
use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static MSGPOOL: Feature = Feature::perf(
    Hosts::SERVER,
    "msgpool",
    "SendMaps message pools",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// Messages in a map thread's pool; other pools are left alone.
const SLOTS: usize = 0x100;
/// Pool (pinned by each platform's signatures): its message array (preceded by its message count, u32), the free
/// list's head (an index, -1: none), and its references (its owner's and one per message taken).
const ARRAY: usize = 0x00;
const HEAD: usize = 0x04;
const REFS: usize = 0x08;
/// Message: its size, and its pool fields: the pool, its index, the next free index, and the taken flag (byte).
const SIZE: usize = 0x43C;
const POOL: usize = 0x42C;
const INDEX: usize = 0x430;
const NEXT: usize = 0x434;
const TAKEN: usize = 0x438;
/// Arrays kept at most (one pool per map thread is in use per tick).
const KEPT: usize = 8;

/// Kept arrays (0: an empty place).
static ARRAYS: [AtomicUsize; KEPT] = [const { AtomicUsize::new(0) }; KEPT];

static BUILT: Counter = Counter::new();
static REUSED: Counter = Counter::new();
static KEPT_ARRAYS: Counter = Counter::new();

/// A kept array, if any.
fn take() -> Option<usize> {
    ARRAYS.iter().find_map(|a| match a.swap(0, Relaxed) {
        0 => None,
        p => Some(p),
    })
}

/// Before the destroy of `pool` (its last reference gone): keeps its array if it's a map thread pool's and there is
/// room, and detaches it from the pool, so the destroy frees only the pool.
///
/// # Safety
/// `pool` is a live pool nothing else refers to.
unsafe extern "C" fn keep(pool: usize) {
    // SAFETY: a live pool (caller); its array is preceded by its count.
    unsafe {
        let array = read::<usize>(pool + ARRAY);
        if array == 0 || read::<u32>(array - 4) as usize != SLOTS || verify() && !verify::all_free(pool, array) {
            return;
        }
        if ARRAYS.iter().any(|a| a.compare_exchange(0, array, Relaxed, Relaxed).is_ok()) {
            write(pool + ARRAY, 0usize);
            KEPT_ARRAYS.bump();
        }
    }
}

/// Links message `i` of a rebuilt array into `pool`'s free list, as the pool constructor does.
///
/// # Safety
/// `m` is message `i` of an array only this thread uses.
unsafe fn link(m: usize, pool: usize, i: usize) {
    // SAFETY: as above.
    unsafe {
        write(m + POOL, pool);
        write(m + INDEX, i as u32);
        write(m + NEXT, if i + 1 < SLOTS { i as i32 + 1 } else { -1 });
        write(m + TAKEN, 0u8);
    }
}

/// Makes `pool` the owner of the rebuilt `array`, as the pool constructor leaves it.
///
/// # Safety
/// `pool` is a pool being constructed.
unsafe fn adopt(pool: usize, array: usize) {
    // SAFETY: as above.
    unsafe {
        write(pool + ARRAY, array);
        write(pool + HEAD, 0u32);
        write(pool + REFS, 1u32);
    }
    REUSED.bump();
}

fn print_stats() {
    eprint!("dmfast: msgpool built={BUILT} reused={REUSED} kept={KEPT_ARRAYS}");
    if verify() {
        eprint!(" mismatches={}", verify::MISMATCHES);
    }
    eprintln!();
}
