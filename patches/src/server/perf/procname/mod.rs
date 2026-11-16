//! `procname`: calling a proc by name (`x.f()`, `call()`, global procs) looks the name up in a sorted array of
//! (name, overrides) entries with a binary search, which misses the cache at most steps on big types (~900 entries).
//! A direct-mapped table remembers where each (array, name) was found, and the lookup checks that entry first.
//!
//! It's exact: an array's names are unique, so the remembered entry is used only if it still holds the name, within
//! the array's count, and is then the entry the search would find; what is returned from it is BYOND's own logic.
//! Anything else runs the search. The callers move, not the function, so tools that find it by its bytes still do.
//! No dependency guard: the signature is the whole lookup.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::{live_u32, read};
use crate::base::driver::verify;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::image::Image;
use crate::base::{Result, ensure};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static PROCNAME: Feature = Feature::perf(
    Hosts::SERVER,
    "procname",
    "proc name lookup",
    install!(windows: windows::install, linux: linux::install),
)
.stats(verify::print_stats);

/// BYOND's lookup: (map, name, depth, flag) -> proc id or 0xFFFF.
type LookupFn = unsafe extern "C" fn(usize, u32, u32, u32) -> u32;

/// The map: entries, then the count (u16). Each entry: name (dword), overrides (dword array), their count (u16),
/// a flag byte.
const ENTRIES: usize = 0;
const COUNT: usize = 4;
const ENTRY: usize = 12;
const OVERRIDES: usize = 4;
const OVERRIDE_COUNT: usize = 8;
const FLAG: usize = 0xA;
const NONE: u32 = 0xFFFF;

const SLOT_BITS: u32 = 12;

/// A remembered lookup: the map, the name, and the index it was found at.
struct Slot {
    map: AtomicU32,
    name: AtomicU32,
    index: AtomicU32,
}

static SLOTS: [Slot; 1 << SLOT_BITS] =
    [const { Slot { map: AtomicU32::new(0), name: AtomicU32::new(0), index: AtomicU32::new(0) } }; 1 << SLOT_BITS];

fn slot(map: usize, name: u32) -> &'static Slot {
    let h = (map as u32 ^ name.wrapping_mul(0x9E37_79B9)).wrapping_mul(0x85EB_CA6B);
    &SLOTS[(h >> (32 - SLOT_BITS)) as usize]
}

/// What BYOND returns for the entry at `e`: the override at `depth`, unless the entry is flagged and `flag` is set,
/// or it has no override that deep.
///
/// # Safety
/// `e` is a live entry.
unsafe fn answer(e: usize, depth: u32, flag: u32) -> u32 {
    // SAFETY: a live entry (caller) and its override array.
    unsafe {
        let depth = depth as u8;
        if read::<u8>(e + FLAG) != 0 && flag as u8 != 0 {
            return NONE;
        }
        if u16::from(depth) >= read::<u16>(e + OVERRIDE_COUNT) {
            return NONE;
        }
        live_u32(live_u32(e + OVERRIDES) as usize + 4 * depth as usize)
    }
}

/// BYOND's binary search: the index of `name` among `count` entries at `entries`, or `None`.
///
/// # Safety
/// `entries` holds `count` live entries.
unsafe fn search(entries: usize, count: u32, name: u32) -> Option<u32> {
    let (mut lo, mut hi) = (0u32, count);
    while lo < hi {
        let mid = (lo + hi) >> 1;
        // SAFETY: mid < count (caller).
        let k = unsafe { live_u32(entries + ENTRY * mid as usize) };
        if name == k {
            return Some(mid);
        }
        if name > k {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}

/// Replaces BYOND's lookup at its callers.
///
/// # Safety
/// `map` is a live map (BYOND passes it).
unsafe extern "C" fn lookup(map: usize, name: u32, depth: u32, flag: u32) -> u32 {
    // SAFETY: a live map (caller): its entries.
    unsafe {
        let entries = live_u32(map + ENTRIES) as usize;
        let count = u32::from(read::<u16>(map + COUNT));
        let s = slot(map, name);
        let i = s.index.load(Relaxed);
        let hit = s.map.load(Relaxed) == map as u32
            && s.name.load(Relaxed) == name
            && i < count
            && live_u32(entries + ENTRY * i as usize) == name;
        let got = if hit {
            answer(entries + ENTRY * i as usize, depth, flag)
        } else {
            match search(entries, count, name) {
                Some(i) => {
                    s.map.store(map as u32, Relaxed);
                    s.name.store(name, Relaxed);
                    s.index.store(i, Relaxed);
                    answer(entries + ENTRY * i as usize, depth, flag)
                }
                None => NONE,
            }
        };
        if verify() {
            verify::check(map, name, depth, flag, got, hit);
        }
        got
    }
}

/// Points the lookup's callers at [`lookup`], except calls in code an earlier feature rewrote or matched.
fn redirect(core: &Image, f: usize) -> Result<String> {
    let calls: Vec<usize> = core.calls_to(f)?.iter().copied().filter(|&c| !core.claimed(c, 5)).collect();
    ensure!(!calls.is_empty(), "proc name lookup: no callers");
    for &c in &calls {
        core.retarget(c + 1, lookup as LookupFn)?;
    }
    verify::ORIGINAL.set(f);
    Ok(core.report(&calls))
}
