//! `turfpf`: software prefetch in SendMaps' turf loop.
//!
//! Each turf in view costs a chain of dependent cache misses (turf hash bucket, chain node, mob and object id lists,
//! each id through its table to the thing). The loop's lookup call goes through a platform stub whose `enter` reads
//! the loop's frame and calls [`prefetch`], which touches the turfs ahead in scan order, the nearest deepest. It's safe
//! because it only reads, with the bounds checks of BYOND's own lookups; the frame values only choose which turfs.
//! `DMFAST_VERIFY=1` also checks those frame values on every call ([`verify::check`]).

use std::arch::x86::{_MM_HINT_T0, _mm_prefetch};

use crate::base::Result;
use crate::base::code::read;
use crate::base::feature::{Ctx, Feature, Hosts, install};
use crate::base::sync::Installed;
use crate::server::layout::THING_LOOK;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static TURFPF: Feature = Feature::perf(
    Hosts::SERVER,
    "turfpf",
    "SendMaps turf prefetch",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// Prefetch stages: the turfs `1 + S*i` ahead (`i < J`), each `J - 1 - i` levels deep.
const J: u32 = 8;
/// Turfs between stages.
const S: u32 = 3;
/// A thing's next id (literal in each platform's loop signature); its appearance: [`THING_LOOK`].
const NEXT: usize = 0x48;
/// A turf's hash chain node: turf id, next node, first object id, and first mob id.
const NODE_NEXT: usize = 0x04;
const NODE_OBJS: usize = 0x08;
const NODE_MOBS: usize = 0x0C;
/// No thing: the end of an id list.
const NONE: u32 = 0xFFFF;

/// BYOND's globals the lookups read, found at install: the lookup's turf count (u32), turf flags (byte array ptr), hash
/// mask (u16), and buckets (pointer array ptr); the mob and object tables (count u32, pointer array ptr); world.maxx
/// (i16); and the lookup, which the stub runs after [`prefetch`].
struct Globals {
    turfs: usize,
    flags: usize,
    mask: usize,
    buckets: usize,
    mobs: (usize, usize),
    objs: (usize, usize),
    maxx: usize,
    lookup: usize,
}

static GLOBALS: Installed<Globals> = Installed::new();

/// Points the lookup call at `call` to `stub` (which calls the original lookup itself).
fn hook(cx: &Ctx, call: usize, globals: Globals, stub: unsafe extern "C" fn()) -> Result<String> {
    let core = cx.core();
    core.untouched(&[(call, 5)])?;
    core.retarget(call + 1, stub)?;
    GLOBALS.commit(cx, globals);
    Ok(format!("{}{}", core.report(&[call]), if crate::base::driver::verify() { " (verify)" } else { "" }))
}

fn touch(addr: usize) {
    // SAFETY: a prefetch hint; it never faults.
    unsafe { _mm_prefetch::<_MM_HINT_T0>(addr as *const i8) }
}

/// Before the lookup: turf id `t` and the loop's frame (`frame` = ebp, turfs left in the row after `t`, row width,
/// rows left). Prefetches the turfs `1 + S*i` ahead (`i < J`) and returns the lookup for the stub to jump to.
#[inline(always)]
fn prefetch(t: u32, frame: usize, rem: u32, width: u32, rows: u32) -> usize {
    let g = GLOBALS.get();
    // SAFETY: BYOND's globals found at install; reads bounds-checked as its lookups.
    unsafe {
        let maxx = read::<i16>(g.maxx) as u32;
        if crate::base::driver::verify() {
            verify::check(maxx, t, frame, rem, width, rows);
        }
        for i in 0..J {
            let k = 1 + S * i;
            let u = t.wrapping_add(k).wrapping_add(if k > rem { maxx.wrapping_sub(width) } else { 0 });
            if u < read::<u32>(g.turfs) && read::<u8>(read::<usize>(g.flags) + u as usize) != 0 {
                let slot = read::<usize>(g.buckets) + (read::<u16>(g.mask) & u as u16) as usize * 4;
                fetch(g, u, slot, J - 1 - i);
            }
        }
        g.lookup
    }
}

/// Prefetches turf `u`'s contents `d` levels past its bucket `slot`: 0 the slot, then its chain nodes, then u's
/// node's first mob and object (table slot, thing), then the next objects.
///
/// # Safety
/// `slot` is `u`'s bucket in the turf hash.
unsafe fn fetch(g: &Globals, u: u32, slot: usize, mut d: u32) {
    if d == 0 {
        return touch(slot);
    }
    // SAFETY: a live bucket and its chain (caller).
    unsafe {
        let mut node = read::<usize>(slot);
        while node != 0 {
            d -= 1;
            if d == 0 {
                return touch(node);
            }
            if read::<u32>(node) == u {
                if d < 3 {
                    things(g.mobs, read(node + NODE_MOBS), d - 1);
                }
                return things(g.objs, read(node + NODE_OBJS), d - 1);
            }
            node = read(node + NODE_NEXT);
        }
    }
}

/// Prefetches `d` levels down the id list from `id` in `table`: 0 the table slot, 1 the thing, 2 the next's slot, ...
///
/// # Safety
/// `count` and `table` are BYOND's mob or object table globals.
unsafe fn things((count, table): (usize, usize), mut id: u32, mut d: u32) {
    // SAFETY: ids bounds-checked against the table's count, as BYOND's by-id lookup does.
    unsafe {
        while id != NONE && id < read::<u32>(count) {
            let slot = read::<usize>(table) + id as usize * 4;
            if d == 0 {
                return touch(slot);
            }
            let thing = read::<usize>(slot);
            if thing == 0 {
                return;
            }
            if d == 1 {
                touch(thing + NEXT);
                return touch(thing + THING_LOOK);
            }
            id = read(thing + NEXT);
            d -= 2;
        }
    }
}

fn print_stats() {
    if GLOBALS.is_set() {
        eprintln!(
            "dmfast: turfpf checked={} followed={} mismatches={}",
            verify::CHECKED,
            verify::FOLLOWED,
            verify::MISMATCHES
        );
    }
}
