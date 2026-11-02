//! `lists`: the compiler's list intern looks lists up in a hash index instead of a binary search tree.
//!
//! The compiler interns int arrays (`id = intern(list)`, the entry's index + 1) through a tree that degrades into long
//! chains. Only the intern reads the tree, so the whole function is replaced: a hash index from contents to id, and
//! on a miss BYOND's insert without the tree link (the same allocators, growth, and ids).
//!
//! It's safe because every insert goes through here, and a hit is checked against the live array (a reset empties
//! it, so a stale key is only a miss). A mutex keeps the index and the array in step. `deps` guards the intern.
//!
//! An entry (18h bytes, from `operator new`): +0 data (malloc), +4 capacity, +8 length (ints), +0Ch index,
//! +10h/+14h tree links (0).

use std::sync::Mutex;

use crate::base::code::{Foreign, MallocFn, ReallocFn, live_u32, write};
use crate::base::driver::Counter;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::hash::{self, Map};
use crate::base::sync::{Installed, lock};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static LISTS: Feature =
    Feature::perf(Hosts::COMPILER, "lists", "list intern", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);

/// The compiler's list globals and the allocators its insert calls.
struct Found {
    /// The globals holding the entry array, its count, and its capacity.
    array: usize,
    count: usize,
    capacity: usize,
    new: Foreign<unsafe extern "C" fn(usize) -> usize>,
    malloc: Foreign<MallocFn>,
    realloc: Foreign<ReallocFn>,
}

static FOUND: Installed<Found> = Installed::new();
/// Contents to id.
static INDEX: Mutex<Map<Box<[u32]>, u32>> = Mutex::new(hash::map());
static HITS: Counter = Counter::new();
static INSERTS: Counter = Counter::new();
static STALE: Counter = Counter::new();

/// The id BYOND never hands out; also what the insert returns when the array can't grow.
const RESERVED: u32 = 0xFFFF;

/// The contents of a list or entry (`+0` data, `+8` length).
///
/// # Safety
/// `list` is a live list or entry.
unsafe fn contents<'a>(list: usize) -> &'a [u32] {
    // SAFETY: a live list (caller): `len` ints at its data.
    unsafe {
        let len = live_u32(list + 8) as usize;
        if len == 0 {
            return &[];
        }
        std::slice::from_raw_parts(live_u32(list) as *const u32, len)
    }
}

/// Whether `id` still names an entry with contents `key`.
///
/// # Safety
/// The globals in `f` are the compiler's.
unsafe fn valid(f: &Found, key: &[u32], id: u32) -> bool {
    // SAFETY: the compiler's array and its entries (caller).
    unsafe {
        if id == 0 || id > live_u32(f.count) {
            return false;
        }
        let e = live_u32(live_u32(f.array) as usize + 4 * (id - 1) as usize) as usize;
        e != 0 && live_u32(e + 0xC) == id - 1 && contents(e) == key
    }
}

/// BYOND's insert without the tree link: the id, or [`RESERVED`] if the array can't grow (as BYOND).
///
/// # Safety
/// The globals and allocators in `f` are the compiler's.
unsafe fn insert(f: &Found, key: &[u32]) -> u32 {
    // SAFETY: the compiler's allocators and globals (caller); the new entry and data are ours to fill.
    unsafe {
        let e = f.new.get()(0x18);
        let data = f.malloc.get()(key.len() * 4) as usize;
        if !key.is_empty() {
            std::ptr::copy_nonoverlapping(key.as_ptr(), data as *mut u32, key.len());
        }
        write(e, data as u32);
        write(e + 4, key.len() as u32);
        write(e + 8, key.len() as u32);
        write(e + 0x10, 0u32);
        write(e + 0x14, 0u32);
        let mut count = live_u32(f.count);
        loop {
            write(e + 0xC, count);
            let mut cap = live_u32(f.capacity);
            if cap <= count {
                cap += (count >> 4) + 0x10;
                write(f.capacity, cap);
                let a = f.realloc.get()(live_u32(f.array) as usize as *mut _, cap as usize * 4) as usize;
                write(f.array, a as u32);
                if a == 0 {
                    return RESERVED;
                }
            }
            write(live_u32(f.array) as usize + 4 * count as usize, e as u32);
            count += 1;
            if count != RESERVED {
                break;
            }
            // Id 0xFFFF is reserved: BYOND clears slot 0xFFFE again and stores the entry once more.
            write(f.count, count);
            write(live_u32(f.array) as usize + 4 * (RESERVED - 1) as usize, 0u32);
        }
        write(f.count, count);
        count
    }
}

/// Replaces the list intern (cdecl: list -> id).
///
/// # Safety
/// `list` is the compiler's list (BYOND passes it).
unsafe extern "C" fn intern(list: usize) -> u32 {
    let f = FOUND.get();
    let mut index = lock(&INDEX);
    // SAFETY: the compiler's list (caller) and globals (checked at install).
    unsafe {
        let key = contents(list);
        if let Some(&id) = index.get(key) {
            if valid(f, key, id) {
                HITS.bump();
                return id;
            }
            STALE.bump();
        }
        INSERTS.bump();
        let id = insert(f, key);
        if id != RESERVED {
            index.insert(key.into(), id);
        }
        id
    }
}

fn print_stats() {
    eprintln!("dmfast: list intern hits={} inserts={} stale={} entries={}", HITS, INSERTS, STALE, lock(&INDEX).len());
}
