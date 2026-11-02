//! `protos`: the compiler's proto lookups are answered from a hash index instead of a red-black tree walk.
//!
//! The compiler interns "protos" (keyed by arrays of 3-dword entries) in a red-black tree whose lookup-or-insert
//! compares key arrays at every level, and most calls find an existing node. A hash index from (tree, key hash) to
//! node answers those. Everything else runs BYOND's lookup, and the index learns its result.
//!
//! It's safe because a hit is used only while the node is still in the tree (its colour isn't 0xFFFF, which unlinking
//! sets) and its key equals the lookup's, and a lookup on an empty tree (also how a reset shows) empties the index.
//! On Linux the hooked unlink also drops the node from the index. Windows unlinks a node only when it got id 0xFFFF,
//! then reuses it for a placeholder at once (the key check rejects it), and frees a pool block only if the last two
//! are empty, which one unlink can't cause. `deps` guards the lookup.
//!
//! A key holder: +1Ch -> key array (+0 entries, +6 u16 count). A node: +18h colour, +1Ch -> key array.

use std::hash::Hasher;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHasher};

use crate::base::code::live_u32;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::lock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static PROTOS: Feature =
    Feature::perf(Hosts::COMPILER, "protos", "proto index", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);

/// A node's index key: its tree and its key's hash.
type Slot = (u32, u64);

struct Index {
    /// Keys are compared on every hit, so a hash collision only costs a fallback.
    nodes: FxHashMap<Slot, u32>,
    /// Each node's index key, to evict it.
    keys: FxHashMap<u32, Slot>,
}

static INDEX: Mutex<Index> =
    Mutex::new(Index { nodes: FxHashMap::with_hasher(FxBuildHasher), keys: FxHashMap::with_hasher(FxBuildHasher) });
static HITS: AtomicU32 = AtomicU32::new(0);
static MISSES: AtomicU32 = AtomicU32::new(0);
static STALE: AtomicU32 = AtomicU32::new(0);
static EVICTED: AtomicU32 = AtomicU32::new(0);
static CLEARED: AtomicU32 = AtomicU32::new(0);

/// The colour unlinking gives a node.
const UNLINKED: u16 = 0xFFFF;

/// The key entries (3 dwords each) of a key holder or node.
///
/// # Safety
/// `holder` is a live key holder or node.
unsafe fn key<'a>(holder: usize) -> &'a [u32] {
    // SAFETY: a live holder (caller): its key array and entries.
    unsafe {
        let array = live_u32(holder + 0x1C) as usize;
        let n = usize::from(((array + 6) as *const u16).read_unaligned()) * 3;
        if n == 0 {
            return &[];
        }
        std::slice::from_raw_parts(live_u32(array) as *const u32, n)
    }
}

fn hash(k: &[u32]) -> u64 {
    let mut h = FxHasher::default();
    k.iter().for_each(|&v| h.write_u32(v));
    h.write_usize(k.len());
    h.finish()
}

/// The node BYOND's lookup would find for `holder`'s key in `tree`, or 0.
///
/// # Safety
/// BYOND's lookup arguments: `tree` is a live tree and `holder` a live key holder.
unsafe extern "C" fn lookup(tree: usize, holder: usize) -> usize {
    let mut index = lock(&INDEX);
    // SAFETY: the caller's tree and holder; indexed nodes are in a live tree (an emptied tree empties the index).
    unsafe {
        // An empty tree (root 0) is also how a reset shows (tree and node pool freed). Every insert goes through
        // this lookup, so emptying the index here drops every entry into freed memory before anything reads one.
        if live_u32(tree) == 0 {
            if !index.nodes.is_empty() {
                index.nodes.clear();
                index.keys.clear();
                CLEARED.fetch_add(1, Relaxed);
            }
            return 0;
        }
        let k = key(holder);
        let Some(&node) = index.nodes.get(&(tree as u32, hash(k))) else {
            MISSES.fetch_add(1, Relaxed);
            return 0;
        };
        let node = node as usize;
        if ((node + 0x18) as *const u16).read_unaligned() == UNLINKED || key(node) != k {
            STALE.fetch_add(1, Relaxed);
            return 0;
        }
        HITS.fetch_add(1, Relaxed);
        node
    }
}

/// BYOND's lookup returned `node` for `holder`'s key in `tree`.
///
/// # Safety
/// `holder` is a live key holder.
unsafe extern "C" fn record(tree: usize, holder: usize, node: usize) {
    if node == 0 {
        return;
    }
    // SAFETY: a live holder (caller).
    let slot = (tree as u32, hash(unsafe { key(holder) }));
    let mut index = lock(&INDEX);
    if let Some(old) = index.nodes.insert(slot, node as u32)
        && old != node as u32
    {
        index.keys.remove(&old);
    }
    index.keys.insert(node as u32, slot);
}

fn print_stats() {
    eprintln!(
        "dmfast: proto index hits={} misses={} stale={} evicted={} cleared={}",
        HITS.load(Relaxed),
        MISSES.load(Relaxed),
        STALE.load(Relaxed),
        EVICTED.load(Relaxed),
        CLEARED.load(Relaxed)
    );
}
