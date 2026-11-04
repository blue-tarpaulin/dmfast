//! `internidx` and `rebuild`: the compiler's identifier intern. It keeps each identifier and path once in a table
//! (id = entry index + 1) and finds them through a binary search tree, a strcmp per level; most interns find an
//! existing string.
//!
//! `internidx`: a hash index from (table, string) to id answers those. A hit is used only if the table confirms it
//! (entry id-1 exists, has index id-1, and holds the string), so a stale entry or a hash collision runs the original,
//! whose id is recorded. Other threads run the original. `deps` guards the intern.
//!
//! `rebuild`: the original rebalances the tree after every 256th insert. Ids are insertion order, never derived from
//! the tree's shape, so rebuilding less often ([`should_rebuild`]) changes only lookup speed.

use std::ffi::{CStr, c_char};
use std::hash::Hasher;

use crate::base::Result;
use crate::base::code::live_u32;
use crate::base::driver::Counter;
use crate::base::feature::{Ctx, Feature, Hosts, install};
use crate::base::hash::{self, FxHasher};
use crate::base::sync::{Installed, Owned};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static REBUILD: Feature = Feature::perf(
    Hosts::COMPILER,
    "rebuild",
    "intern rebuild",
    install!(windows: windows::install_rebuild, linux: linux::install_rebuild),
)
.stats(|| eprintln!("dmfast: intern tree rebuilds={REBUILDS}"));

pub static INTERNIDX: Feature = Feature::perf(
    Hosts::COMPILER,
    "internidx",
    "intern index",
    install!(windows: windows::install_index, linux: linux::install_index),
)
.stats(print_stats);

/// The table: its entry array and entry count (the intern's tree root is at +80h).
const ENTRIES: usize = 0x84;
const COUNT: usize = 0x88;
/// An entry: its index in the array, and its string (inline, NUL-terminated).
const ENTRY_INDEX: usize = 0;
const ENTRY_STRING: usize = 0xC;

/// The index, owned by the compiler's thread; the counts are plain adds on the hot path.
struct Index {
    /// hash(table, string) -> id; hits compare the strings, so a collision only falls back.
    ids: hash::Map<u32, u32>,
    /// A hit verify holds back for the original to confirm: (hash, id).
    held: Option<(u32, u32)>,
    hits: u64,
    misses: u64,
    stale: u64,
}

static INDEX: Owned<Index> = Owned::new();
/// Rebuild after count/K inserts at most (a power of two; `DMFAST_INTERN_K`, default 8).
static K: Installed<u32> = Installed::new();
static REBUILDS: Counter = Counter::new();

/// Set the index (its stub's install, once everything is checked).
fn init_index() -> Result<()> {
    INDEX.set(Index { ids: hash::map(), held: None, hits: 0, misses: 0, stale: 0 })
}

/// Read `DMFAST_INTERN_K` for [`should_rebuild`] (its stub's install): K.
fn init_rebuild(cx: &Ctx) -> u32 {
    let k = std::env::var("DMFAST_INTERN_K")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|k| k.is_power_of_two())
        .unwrap_or(8);
    K.commit(cx, k);
    k
}

fn hash(table: usize, s: &[u8]) -> u32 {
    let mut h = FxHasher::default();
    h.write_usize(table);
    h.write(s);
    h.finish() as u32
}

/// Whether `table`'s entry for `id` exists, has index id-1, and holds `key`.
///
/// # Safety
/// `table` is a live intern table.
unsafe fn confirms(table: usize, id: u32, key: &[u8]) -> bool {
    // SAFETY: a live table (caller): ids up to its count index its entry array; entry strings are NUL-terminated
    // and read only up to the first difference.
    unsafe {
        if id == 0 || id > live_u32(table + COUNT) {
            return false;
        }
        let e = live_u32(live_u32(table + ENTRIES) as usize + 4 * (id - 1) as usize) as usize;
        let s = (e + ENTRY_STRING) as *const u8;
        e != 0
            && live_u32(e + ENTRY_INDEX) == id - 1
            && key.iter().enumerate().all(|(i, &b)| *s.add(i) == b)
            && *s.add(key.len()) == 0
    }
}

/// The id the intern would return for (table, string) if the index knows it and the table confirms it; 0 runs the
/// original. Called by the intern stubs.
///
/// # Safety
/// The intern's arguments: `table` is a live intern table and `s` a NUL-terminated string.
unsafe extern "C" fn lookup(table: usize, s: *const c_char) -> u32 {
    let Some(ix) = INDEX.get() else { return 0 };
    // SAFETY: the intern's string (caller).
    let key = unsafe { CStr::from_ptr(s) }.to_bytes();
    let h = hash(table, key);
    let Some(&id) = ix.ids.get(&h) else {
        ix.misses += 1;
        return 0;
    };
    // SAFETY: a live table (caller).
    if !unsafe { confirms(table, id, key) } {
        ix.stale += 1;
        return 0;
    }
    ix.hits += 1;
    if verify::hold(ix, h, id) {
        return 0;
    }
    id
}

/// The original intern returned `id` for (table, string). Called by the intern stubs.
///
/// # Safety
/// `s` is a NUL-terminated string.
unsafe extern "C" fn record(table: usize, s: *const c_char, id: u32) {
    let Some(ix) = INDEX.get() else { return };
    // SAFETY: a NUL-terminated string (caller).
    let h = hash(table, unsafe { CStr::from_ptr(s) }.to_bytes());
    verify::check(ix, h, id);
    ix.ids.insert(h, id);
}

/// Whether to rebuild the tree now that it holds `count` entries: when `count` is a multiple of max(256, the
/// largest power of two up to `count` / K), so a subset of the original's every 256th. Called by the rebuild stubs.
extern "C" fn should_rebuild(count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    let pow2 = 1u32 << (31 - count.leading_zeros());
    let yes = count.is_multiple_of((pow2 / *K.get()).max(256));
    if yes {
        REBUILDS.bump();
    }
    u32::from(yes)
}

fn print_stats() {
    // SAFETY: exit-time stats: the compiler no longer runs the hooks.
    let Some(ix) = (unsafe { INDEX.shared() }) else { return };
    let (hits, misses, stale) = (ix.hits, ix.misses, ix.stale);
    eprintln!(
        "dmfast: intern index hits={hits} misses={misses} stale={stale} verify={} mismatches={}",
        crate::base::driver::verify(),
        verify::MISMATCHES
    );
}
