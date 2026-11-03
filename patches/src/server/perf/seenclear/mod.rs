//! `seenclear`: SendMaps' per-tick clear of the "seen this scan" flag on a client's record tree as a sweep of the
//! tree's node pool in memory order, not a walk of its list in id order.
//!
//! It's safe because the pool's in-use slots are exactly the list's nodes: each tree allocates its nodes from its own
//! pool and links every one into the list at once, and a free slot's word at [`COLOUR`] is FFFFh (set by the node
//! constructor and the free; an allocated node holds its red-black colour, 0 or 1, there). So no other tree's flags
//! are touched. `DMFAST_VERIFY=1` runs both from the same flags on every call and compares every slot.
//!
//! Dependency guards (`deps`): the pool's allocator (its callers) and its frees.

use crate::base::code::{read, write};
use crate::base::driver::verify;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Installed;
use crate::server::layout::{RECORD_FLAGS, SEEN};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static SEENCLEAR: Feature = Feature::perf(
    Hosts::SERVER,
    "seenclear",
    "SendMaps seen-flag clear",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// Node size, as the allocator's `imul 5Ch` (checked at install).
const NODE: usize = 0x5C;
/// Tree header: the first node in id order, the chunk count, the chunk array ({first slot, live nodes} each), and the
/// slots per chunk.
const FIRST: usize = 0x04;
const NCHUNKS: usize = 0x14;
const CHUNKS: usize = 0x18;
const SLOTS: usize = 0x20;
/// Node: the next in id order, and the red-black colour ([`FREE`]: a free slot).
const NEXT: usize = 0x0C;
const COLOUR: usize = 0x18;
const FREE: u16 = 0xFFFF;

/// Set when the install commits.
static INSTALLED: Installed<()> = Installed::new();

/// Calls `f(slot, in use)` on each slot of the chunks with live nodes; unless `all`, a chunk stops after its last live
/// node.
///
/// # Safety
/// `tree` is a live record tree.
#[inline(always)]
unsafe fn each_slot(tree: usize, all: bool, mut f: impl FnMut(usize, bool)) {
    // SAFETY: a live tree (caller): `NCHUNKS` chunks of `SLOTS` nodes each.
    unsafe {
        let (chunks, slots) = (read::<u32>(tree + CHUNKS) as usize, read::<u32>(tree + SLOTS));
        for c in 0..read::<u32>(tree + NCHUNKS) as usize {
            let (mut node, mut live) = (read::<u32>(chunks + 8 * c) as usize, read::<u32>(chunks + 8 * c + 4));
            if live == 0 || node == 0 {
                continue;
            }
            for _ in 0..slots {
                let used = read::<u16>(node + COLOUR) != FREE;
                f(node, used);
                if used && !all {
                    live -= 1;
                    if live == 0 {
                        break;
                    }
                }
                node += NODE;
            }
        }
    }
}

/// Clears the seen flag on every in-use node of the tree's pool.
///
/// # Safety
/// `tree` is a live record tree.
#[inline(always)]
unsafe fn sweep(tree: usize) {
    // SAFETY: a live tree (caller).
    unsafe {
        each_slot(tree, false, |node, used| {
            if used {
                write(node + RECORD_FLAGS, read::<u16>(node + RECORD_FLAGS) & !SEEN);
            }
        })
    }
}

/// Replaces BYOND's clear (cdecl `tree`); returns 0, as the original leaves eax.
///
/// # Safety
/// `tree` is a live client record tree; only its owner's thread uses it during the call.
unsafe extern "C" fn clear_seen(tree: usize) -> u32 {
    // SAFETY: a live tree only this thread uses (caller).
    unsafe {
        if verify() {
            verify::clear(tree);
        } else {
            sweep(tree);
        }
    }
    0
}

fn print_stats() {
    if INSTALLED.is_set() && verify() {
        eprintln!("dmfast: seenclear verified={} mismatches={}", verify::CALLS, verify::MISMATCHES);
    }
}
