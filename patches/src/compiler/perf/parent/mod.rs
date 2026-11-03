//! `parent`: the parent lookup cache. Name resolution asks for a node's parent type at every step, and BYOND's lookup
//! scans the node's children backwards for the last `..` link (type 0x45). For nodes with at least [`MIN_CHILDREN`]
//! children, a direct-mapped table keyed by a hash of the node's address keeps that child's index.
//!
//! It's safe because children are only appended, so an entry keyed by node, children array, and count stays valid;
//! each platform checks more and drops a node's entry when its pool reuses or frees it. Smaller nodes and other
//! threads run the scan. `deps` guards the lookup.

use crate::base::code::live_u32;
use crate::base::feature::{Feature, Hosts, install};
use crate::compiler::layout::ID;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::CACHE;
#[cfg(windows)]
use windows::CACHE;

pub static PARENT: Feature = Feature::perf(
    Hosts::COMPILER,
    "parent",
    "parent cache",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// The `..` child's node id.
const TYPE_PARENT_LINK: u32 = 0x45;
/// Smaller nodes are cheaper to scan than to look up.
const MIN_CHILDREN: u16 = 16;
/// No `..` child: a sentinel rather than an `Option`, which would grow the entries Windows' eviction stub reads.
const NONE: u16 = u16::MAX;
/// One slot per hash of the node address; a collision replaces the slot.
const SLOT_BITS: u32 = 17;

/// The table and its counts, owned by the compiler's thread. The counts are `u64`: a big compile looks up billions
/// of parents, and plain adds keep the hot path free of atomics.
struct Cache<E> {
    slots: Box<[E]>,
    hits: u64,
    misses: u64,
}

fn hash(node: u32) -> u32 {
    node.wrapping_mul(0x9E37_79B1)
}

fn slot(node: u32) -> usize {
    (hash(node) >> (32 - SLOT_BITS)) as usize
}

/// The id of child `i` in `children`.
///
/// # Safety
/// `children[i]` is a live node.
unsafe fn tag(children: usize, i: u16) -> u32 {
    // SAFETY: a live child (caller).
    unsafe { live_u32(live_u32(children + 4 * i as usize) as usize + ID) }
}

/// Index of the last `..` child, or [`NONE`]: BYOND's scan.
///
/// # Safety
/// `children` holds `count` live nodes.
unsafe fn scan(children: usize, count: u16) -> u16 {
    // SAFETY: i < count: live children (caller).
    (0..count).rev().find(|&i| unsafe { tag(children, i) } == TYPE_PARENT_LINK).unwrap_or(NONE)
}

fn print_stats() {
    // SAFETY: exit-time stats: the compiler no longer runs the hooks.
    if let Some(c) = unsafe { CACHE.shared() } {
        eprintln!(
            "dmfast: parent cache hits={} misses={} verify={} mismatches={}",
            c.hits,
            c.misses,
            crate::base::driver::verify(),
            verify::MISMATCHES
        );
    }
}
