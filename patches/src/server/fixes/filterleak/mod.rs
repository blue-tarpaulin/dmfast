//! `filterleak`: writing a var on a filter (`o.filters[1]:size = 5`) no longer leaks memory.
//!
//! The write builds the atom's new filter chain from a copy of the old one. BYOND keeps freed chains on a free list
//! that its other chain builders take from first, but this copy always allocates, and when the new chain already
//! exists its block goes onto the free list: one block more each write, never handed back. The fix takes the copy's
//! block from the free list when it has one, as BYOND's other chain builders do.
//!
//! Safe: the chain is the same; only which block holds it changes.

use crate::base::code::{self, Foreign};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static FILTERLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "filterleak",
    "a filter var write reuses a freed filter chain block",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// A chain block's size; its next free block is at +8.
const BLOCK: usize = 0xC;
const NEXT: usize = 8;

/// BYOND's chain copy (cdecl: the chain; a new block), `free`, and the free list head.
struct Found {
    copy: Foreign<unsafe extern "C" fn(usize) -> usize>,
    free: Foreign<unsafe extern "C" fn(usize)>,
    head: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// In place of the filter var write's chain copy: the copy, moved into a block from the free list if there is one.
extern "C" fn copy(chain: usize) -> usize {
    let found = FOUND.get();
    // SAFETY: BYOND's copy, free list, and `free`, on the server thread as the write runs.
    unsafe {
        let fresh = found.copy.get()(chain);
        let block = code::read::<usize>(found.head);
        if fresh == 0 || block == 0 {
            return fresh;
        }
        code::write::<usize>(found.head, code::read::<usize>(block + NEXT));
        std::ptr::copy_nonoverlapping(fresh as *const u8, block as *mut u8, BLOCK);
        found.free.get()(fresh);
        block
    }
}
