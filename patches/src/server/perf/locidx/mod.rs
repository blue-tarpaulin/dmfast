//! `locidx`: `view()`, `range()`, `orange()`, `oview()`, and the hearers family without their quadratic dedup.
//!
//! They build their result in a vector {data, capacity, count} through one append ("Overflow in AddToLocList"),
//! adding each atom of a turf's or area's contents or overlap list only if a scan of the whole vector misses it.
//! The platform code replaces those scans with [`contains`], a hash set of the vector being built, and hooks the
//! append ([`entry`]) so the set follows each element added. It's safe because the set is only trusted for the vector
//! it last saw: a lookup on another vector, or after a change the set didn't see (count, data pointer), rebuilds it,
//! and an append to the tracked vector that doesn't continue what the set saw stops tracking. Vectors shorter than
//! [`SMALL`] and threads other than the world's get the stock scan. `DMFAST_VERIFY=1` also runs the stock scan,
//! answers with it, and counts disagreements.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::base::code::Foreign;
use crate::base::driver::Counter;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Owned;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static LOCIDX: Feature = Feature::perf(
    Hosts::SERVER,
    "locidx",
    "view/range dedup index",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// The append (cdecl): value lo, hi, &data, &capacity, &count.
type AppendFn = unsafe extern "C" fn(u32, u32, *mut *mut u32, *mut u32, *mut u32) -> u32;

/// Below this many elements the stock scan is cheaper than keeping the set.
const SMALL: u32 = 256;

/// The original append, set at install.
static ORIGINAL: Foreign<AppendFn> = Foreign::new();
/// The tracked vector's &data (0: none), compared by [`entry`].
static TRACKED: AtomicUsize = AtomicUsize::new(0);
static INDEX: Owned<Index> = Owned::new();
static LOOKUPS: Counter = Counter::new();
static REBUILDS: Counter = Counter::new();
/// Lookups from other threads (stock scan).
static FOREIGN: Counter = Counter::new();

/// A vector as the set last saw it.
#[derive(Clone, Copy, PartialEq)]
struct Seen {
    pdata: usize,
    pcount: usize,
    data: usize,
    count: u32,
}

#[derive(Default)]
struct Index {
    /// The vector the set holds; `None`: none.
    seen: Option<Seen>,
    /// [`key`]s of its elements.
    set: Set,
}

/// What the scans compare: the type byte and the data word.
fn key(ty: u32, data: u32) -> u64 {
    (ty as u8 as u64) << 32 | data as u64
}

/// A set of [`key`]s: open addressing with linear probing; a slot is live if it holds the set's generation. Not
/// `hash::Set`: clearing between vectors is one increment here, not a pass over the whole capacity.
#[derive(Default)]
struct Set {
    slots: Vec<(u32, u64)>,
    used: usize,
    generation: u32,
}

impl Set {
    /// The slot holding `k`, or the free one where it would go.
    fn find(&self, k: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut j = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
        while self.slots[j].0 == self.generation && self.slots[j].1 != k {
            j = (j + 1) & mask;
        }
        j
    }

    fn contains(&self, k: u64) -> bool {
        self.slots[self.find(k)].0 == self.generation
    }

    /// Inserts `k`, doubling the slots (at least 1024) when half full.
    fn insert(&mut self, k: u64) {
        if self.used * 2 >= self.slots.len() {
            let n = (self.slots.len() * 2).max(1024);
            let old = std::mem::replace(&mut self.slots, vec![(0, 0); n]);
            let live = std::mem::replace(&mut self.generation, 1);
            self.used = 0;
            old.into_iter().filter(|s| s.0 == live).for_each(|s| self.insert(s.1));
        }
        let j = self.find(k);
        if self.slots[j].0 != self.generation {
            self.slots[j] = (self.generation, k);
            self.used += 1;
        }
    }

    /// Empties it: a new generation; on wrap, every slot is reset first.
    fn clear(&mut self) {
        self.used = 0;
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.slots.fill((0, 0));
            self.generation = 1;
        }
    }
}

/// Replaces the unique-append scans (cdecl): nonzero if the vector already holds (type byte, data).
///
/// # Safety
/// `pdata`/`pcount` point at a live vector's data pointer and count, as the stock scan reads them.
unsafe extern "C" fn contains(ty: u32, val: u32, pdata: *const *const u32, pcount: *const u32) -> u32 {
    // SAFETY: a live vector (caller): `count` elements of two words at `data`.
    let (data, count, k) = unsafe { (*pdata, *pcount, key(ty, val)) };
    // SAFETY: as above, `i < count`.
    let at = |i: usize| unsafe { key(*data.add(2 * i), *data.add(2 * i + 1)) };
    let stock = || (0..count as usize).any(|i| at(i) == k);
    if count < SMALL {
        return stock() as u32;
    }
    let Some(x) = INDEX.get() else {
        FOREIGN.bump();
        return stock() as u32;
    };
    let seen = Seen { pdata: pdata as usize, pcount: pcount as usize, data: data as usize, count };
    if x.seen != Some(seen) {
        REBUILDS.bump();
        x.set.clear();
        (0..count as usize).for_each(|i| x.set.insert(at(i)));
        x.seen = Some(seen);
        TRACKED.store(pdata as usize, Relaxed);
    }
    LOOKUPS.bump();
    verify::answer(x.set.contains(k), stock) as u32
}

/// The append for the tracked vector (from [`entry`]): the original, then the set follows; eax passed through.
///
/// # Safety
/// As the original's call.
unsafe extern "C" fn add(lo: u32, hi: u32, pdata: *mut *mut u32, pcap: *mut u32, pcount: *mut u32) -> u32 {
    // SAFETY: the append's arguments (caller): a live vector, grown by the original (found at install).
    unsafe {
        let original = ORIGINAL.get();
        let Some(x) = INDEX.get() else { return original(lo, hi, pdata, pcap, pcount) };
        let n = *pcount;
        let at = Seen { pdata: pdata as usize, pcount: pcount as usize, data: *pdata as usize, count: n };
        let follows = x.seen == Some(at);
        let ret = original(lo, hi, pdata, pcap, pcount);
        let data = *pdata;
        if follows && *pcount == n + 1 && *data.add(2 * n as usize) == lo && *data.add(2 * n as usize + 1) == hi {
            x.set.insert(key(lo, hi));
            x.seen = Some(Seen { data: data as usize, count: n + 1, ..at });
        } else {
            x.seen = None;
            TRACKED.store(0, Relaxed);
        }
        ret
    }
}

/// The append's entry hook (cdecl: value lo, hi, &data, &capacity, &count): jumps to [`add`] for the tracked vector,
/// else to the original. Asm: an `extern "C"` fn would save registers and load all arguments on every append.
///
/// # Safety
/// Only jumped to from the append's start.
#[unsafe(naked)]
unsafe extern "C" fn entry() {
    naked_asm!(
        "mov eax, dword ptr [esp + 12]",
        "cmp eax, dword ptr [{tracked}]",
        "je {add}",
        "jmp dword ptr [{original}]",
        tracked = sym TRACKED,
        add = sym add,
        original = sym ORIGINAL,
    )
}

/// Records the install: the original append, and the set's state.
fn init(original: usize) -> crate::base::Result<()> {
    ORIGINAL.set(original);
    INDEX.set(Index::default())
}

fn print_stats() {
    // SAFETY: only whether it is set.
    if unsafe { INDEX.shared() }.is_some() {
        eprintln!(
            "dmfast: locidx lookups={LOOKUPS} rebuilds={REBUILDS} other-thread={FOREIGN} mismatches={}",
            verify::MISMATCHES
        );
    }
}
