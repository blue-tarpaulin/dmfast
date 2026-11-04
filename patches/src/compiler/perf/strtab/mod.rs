//! `strtab`: the string intern's index. The compiler's string intern (`id = intern(entry, keep)`) walks an unbalanced
//! binary tree rooted at slot 0's entry, a strcmp per level. Instead a hash index answers hits; misses run the
//! original insert past its walk, which fills the same slot but doesn't link the new entry into the tree; and
//! [`relink`] links every entry into a balanced tree before the `.dmb` save, the tree's only other reader.
//!
//! It's safe because ids are slots, never derived from the tree's shape, and a hit is used only if its slot holds the
//! string. `DMFAST_VERIFY=1` walks and keeps BYOND's tree and checks the index against it. `deps` guards the intern,
//! the save, and the table reset (Linux: the compare too).

use std::hash::Hasher;
use std::sync::Mutex;
use std::time::Instant;

use crate::base::code::{Foreign, live_u32, read, write};
use crate::base::driver::verify;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::hash::{self, FxHasher};
use crate::base::sync::{Installed, lock};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static STRTAB: Feature = Feature::perf(
    Hosts::COMPILER,
    "strtab",
    "string index",
    install!(windows: windows::install, linux: linux::install),
)
// Wraps ftell's hook on the save's entry (Windows).
.after(&[&crate::compiler::perf::stdio::FTELL])
.stats(print_stats);

/// An entry (32 bytes): its string (a `char*`), id, tree links, reference count, and flag byte.
const STRING: usize = 0;
const ID: usize = 4;
const LEFT: usize = 8;
const RIGHT: usize = 0xC;
const REFS: usize = 0x10;
const FLAG: usize = 0x1D;

/// The original insert past its walk: cdecl `(entry, keep) -> id`.
type InsertFn = unsafe extern "C" fn(usize, *const u8) -> u32;

/// What install found.
struct Found {
    /// The globals holding the table's address and its count.
    table: usize,
    count: usize,
    /// The C runtime's `free`, the one the hit path calls.
    free: Foreign<unsafe extern "C" fn(usize)>,
    insert: Foreign<InsertFn>,
}

static FOUND: Installed<Found> = Installed::new();

/// The index and its counts. A string whose hash another live string holds goes to `collided`.
struct Index {
    ids: hash::Map<u64, u32>,
    collided: hash::Map<Box<[u8]>, u32>,
    /// Entries were added since the last [`relink`] (not linked into the tree).
    unlinked: bool,
    hits: u64,
    inserts: u64,
    stale: u64,
    rebuilds: u64,
    rebuild_us: u64,
    mismatches: u64,
}

static INDEX: Mutex<Index> = Mutex::new(Index {
    ids: hash::map(),
    collided: hash::map(),
    unlinked: false,
    hits: 0,
    inserts: 0,
    stale: 0,
    rebuilds: 0,
    rebuild_us: 0,
    mismatches: 0,
});

/// The string of `entry`.
///
/// # Safety
/// `entry` is a live entry.
unsafe fn string<'a>(entry: usize) -> &'a [u8] {
    // SAFETY: a live entry's string is NUL-terminated (caller).
    unsafe { std::ffi::CStr::from_ptr(live_u32(entry + STRING) as *const std::ffi::c_char).to_bytes() }
}

fn hash(s: &[u8]) -> u64 {
    let mut h = FxHasher::default();
    h.write(s);
    h.finish()
}

impl Found {
    /// The entry in slot `id`, if any.
    ///
    /// # Safety
    /// The table's globals (install).
    unsafe fn entry(&self, id: u32) -> Option<usize> {
        // SAFETY: within the count (caller: the table's globals).
        unsafe {
            if id >= live_u32(self.count) {
                return None;
            }
            Some(live_u32(live_u32(self.table) as usize + 4 * id as usize) as usize).filter(|&e| e != 0)
        }
    }

    /// The entry in slot `id` if it holds `key`.
    ///
    /// # Safety
    /// As [`Found::entry`].
    unsafe fn holding(&self, id: u32, key: &[u8]) -> Option<usize> {
        // SAFETY: a live entry (caller).
        unsafe { self.entry(id).filter(|&e| string(e) == key) }
    }
}

/// Replaces the intern (cdecl).
///
/// # Safety
/// As the original: `entry` is the new entry, `keep` points at its keep flag.
unsafe extern "C" fn intern(entry: usize, keep: *const u8) -> u32 {
    let f = FOUND.get();
    let mut ix = lock(&INDEX);
    // SAFETY: the caller's entry and the table's live entries (install).
    unsafe {
        let key = string(entry);
        let h = hash(key);
        let mut found = ix.ids.get(&h).and_then(|&id| f.holding(id, key));
        if found.is_none() && !ix.collided.is_empty() {
            found = ix.collided.get(key).and_then(|&id| f.holding(id, key));
        }
        let mut link = None;
        if verify() {
            (found, link) = verify::walk(f, &mut ix, key, found);
        }
        if let Some(e) = found {
            ix.hits += 1;
            // The original's hit path: free the new entry's string unless kept, and pass on its flag.
            if read::<u8>(keep as usize) == 0 {
                f.free.get()(live_u32(entry + STRING) as usize);
                write::<u32>(entry + STRING, 0);
            }
            if read::<u8>(entry + FLAG) != 0 {
                write::<u8>(e + FLAG, 1);
                write::<u32>(e + REFS, 0x8000_0000);
            }
            return live_u32(e + ID);
        }
        if ix.ids.contains_key(&h) {
            ix.stale += 1;
        }
        ix.inserts += 1;
        let id = f.insert.get()(entry, keep);
        let Some(new) = f.entry(id) else { return id };
        // A hash still held by another live string (whose contents differ: it missed) stays with it.
        if ix.ids.get(&h).is_some_and(|&other| other != id && f.entry(other).is_some()) {
            ix.collided.insert(string(new).into(), id);
        } else {
            ix.ids.insert(h, id);
        }
        if !verify() {
            ix.unlinked = true;
        } else if let Some(link) = link {
            write::<u32>(link, new as u32);
        }
        id
    }
}

/// At the save's entry: link every entry into a balanced tree rooted at slot 0's, if some aren't.
extern "C" fn relink() {
    let mut ix = lock(&INDEX);
    if !ix.unlinked {
        return;
    }
    ix.unlinked = false;
    ix.rebuilds += 1;
    let t0 = Instant::now();
    // SAFETY: the table's globals (install); the save hasn't started.
    unsafe { rebuild(FOUND.get()) };
    ix.rebuild_us += t0.elapsed().as_micros() as u64;
}

/// # Safety
/// The table's globals (install); no one walks the tree meanwhile.
unsafe fn rebuild(f: &Found) {
    // SAFETY: live entries (caller).
    unsafe {
        let Some(root) = f.entry(0) else {
            // No root: every walk sees an empty tree, as it would have.
            for e in (0..live_u32(f.count)).filter_map(|i| f.entry(i)) {
                write::<u32>(e + LEFT, 0);
                write::<u32>(e + RIGHT, 0);
            }
            return;
        };
        // strcmp compares bytes unsigned, as slices do; a big-endian prefix decides most comparisons.
        let prefix = |k: &[u8]| {
            let mut b = [0u8; 8];
            let n = k.len().min(8);
            b[..n].copy_from_slice(&k[..n]);
            u64::from_be_bytes(b)
        };
        let mut keyed: Vec<(u64, &[u8], usize)> = (1..live_u32(f.count))
            .filter_map(|i| f.entry(i))
            .filter(|&e| e != root)
            .map(|e| {
                let k = string(e);
                (prefix(k), k, e)
            })
            .collect();
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        let nodes: Vec<(&[u8], usize)> = keyed.into_iter().map(|(_, k, e)| (k, e)).collect();
        let split = nodes.partition_point(|&(k, _)| k < string(root));
        write::<u32>(root + LEFT, build(&nodes[..split]));
        write::<u32>(root + RIGHT, build(&nodes[split..]));
    }
}

/// Links `nodes` (sorted) into a balanced subtree; returns its root.
///
/// # Safety
/// Live entries.
unsafe fn build(nodes: &[(&[u8], usize)]) -> u32 {
    if nodes.is_empty() {
        return 0;
    }
    let mid = nodes.len() / 2;
    let e = nodes[mid].1;
    // SAFETY: live entries (caller).
    unsafe {
        write::<u32>(e + LEFT, build(&nodes[..mid]));
        write::<u32>(e + RIGHT, build(&nodes[mid + 1..]));
    }
    e as u32
}

fn print_stats() {
    let ix = lock(&INDEX);
    eprintln!(
        "dmfast: string index hits={} inserts={} stale={} rebuilds={} ({} ms) verify={} mismatches={}",
        ix.hits,
        ix.inserts,
        ix.stale,
        ix.rebuilds,
        ix.rebuild_us / 1000,
        verify(),
        ix.mismatches
    );
}
