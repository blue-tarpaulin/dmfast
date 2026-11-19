//! `callextleak` (forum report 2991331): `call_ext()` with `arglist()` no longer leaks a reference to every arg.
//!
//! Both call_ext() opcodes that take an `arglist()` (by library and function name, and by `load_ext()` handle) copy
//! the list's items into a buffer, referencing each, but the buffer's cleanup does nothing, so every item stays
//! referenced for good. A list of more than 10 items goes into a block the buffer allocates and never frees. BYOND
//! 516.1689 releases a short list's items after a call that returns, but not a long list's, nor when the call throws
//! (fails, or awaits on older builds). The fix notes a long list's block and item count in the buffer's unused inline
//! slots, and the cleanup, after the call and in the handler for a throwing call, releases the items and frees the
//! block.
//!
//! Safe: the cleanup runs when the call is done with the buffer, and the items were referenced for it alone.

use crate::base::code::{self, Foreign};
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};
use crate::server::values::{self, ReleaseFn, Slot};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

pub static CALLEXTLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "callextleak",
    "call_ext()(arglist()) releases its args",
    Tier::Safe,
    install!(windows: install, linux: install),
);

/// The buffer's allocation of room for `count` values (cdecl: the buffer, the count): its inline slots while they
/// last, else a new block.
type AllocFn = unsafe extern "C" fn(usize, u32) -> usize;

/// BYOND's allocation and value release, the C runtime's `free`, and the buffer's fields: the count of inline slots
/// in use, and a block's capacity (0: inline).
struct Found {
    alloc: Foreign<AllocFn>,
    release: Foreign<ReleaseFn>,
    free: Foreign<unsafe extern "C" fn(usize)>,
    used: usize,
    block: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// In place of the opcodes' allocation: the allocation, and for a block, its address and the item count in the
/// buffer's first inline slot, which a block leaves unused.
extern "C" fn alloc(buffer: usize, count: u32) -> usize {
    let f = FOUND.get();
    // SAFETY: BYOND's allocation for its buffer; the inline slot is the buffer's own.
    unsafe {
        let at = f.alloc.get()(buffer, count);
        if code::read::<u32>(buffer + f.block) != 0 {
            code::write::<Slot>(buffer, [at as u32, count]);
        }
        at
    }
}

/// In place of the buffer's cleanup (which does nothing): release the items, free a block, and mark the buffer
/// empty.
extern "C" fn cleanup(buffer: usize) {
    let f = FOUND.get();
    // SAFETY: the opcode's buffer once the call is done; its items are the values [`alloc`] noted, or the inline ones.
    unsafe {
        if code::read::<u32>(buffer + f.block) != 0 {
            let [at, count] = code::read::<Slot>(buffer);
            f.release.get()(at as usize as *mut Slot, count);
            f.free.get()(at as usize);
            code::write::<u32>(buffer + f.block, 0);
        } else {
            f.release.get()(buffer as *mut Slot, code::read::<u32>(buffer + f.used));
        }
        code::write::<u32>(buffer + f.used, 0);
    }
}

/// The dword at `at` as shipped, not relied on.
fn shipped_u32(core: &Image, at: usize) -> Result<u32> {
    let b = core.haystack(at, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Where a `call rel32` at `call` goes, as shipped, not relied on.
fn shipped_target(core: &Image, call: usize) -> Result<usize> {
    Ok((call + 5).wrapping_add(shipped_u32(core, call + 1)? as i32 as usize))
}

/// The opcodes' calls to the allocation (the buffer as a frame offset at +2 of [`os::ALLOC_CALL`]), and the frame
/// offsets.
fn allocs(core: &Image, f: usize) -> Result<(Vec<usize>, Vec<u32>)> {
    let back = os::ALLOC_CALL.split(' ').count() - 1;
    let mut calls = Vec::new();
    let mut buffers = Vec::new();
    for &at in core.probe_calls_to(f)?.iter() {
        if core.is_at(at - back, os::ALLOC_CALL)? {
            calls.push(at);
            buffers.push(core.u32_at(at - back + 2)?);
        }
    }
    ensure!((1..=2).contains(&calls.len()), "call_ext(arglist()): {} buffer allocations", calls.len());
    Ok((calls, buffers))
}

/// The calls to the buffers' cleanup: of the [`os::CLEANUP`] calls for one of `buffers` (at +2) to a bare return, the
/// ones to the function the first allocation's buffer is cleaned up with next. Each buffer must have one.
fn cleanups(core: &Image, first: usize, buffers: &[u32]) -> Result<Vec<usize>> {
    let mut found = Vec::new();
    for pat in os::CLEANUP {
        let len = pat.split(' ').count();
        for &at in core.probe_all(pat)?.iter() {
            if !buffers.contains(&shipped_u32(core, at + 2)?) {
                continue;
            }
            let call = at + len - 1;
            let target = shipped_target(core, call)?;
            if core.haystack(target, 3).is_ok_and(os::is_ret) {
                found.push((at, len, call, target));
            }
        }
    }
    found.sort_unstable();
    let (_, _, _, cleanup) = *found
        .iter()
        .find(|&&(at, ..)| at > first && at < first + 0x400)
        .ok_or("call_ext(arglist()): no buffer cleanup after the allocation")?;
    let mut calls = Vec::new();
    let mut cleaned = Vec::new();
    for &(at, len, call, target) in found.iter().filter(|&&(.., t)| t == cleanup) {
        core.rely(at, len + 4)?;
        core.rely(target, 1)?;
        calls.push(call);
        cleaned.push(core.u32_at(at + 2)?);
    }
    ensure!(buffers.iter().all(|b| cleaned.contains(b)), "call_ext(arglist()): a buffer without cleanup");
    Ok(calls)
}

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(os::ALLOC, "call_ext(arglist()) buffer allocation")?;
    let free = os::free(core, f)?;
    let (calls, buffers) = allocs(core, f)?;
    let cleanups = cleanups(core, calls[0], &buffers)?;
    FOUND.commit(
        cx,
        Found {
            alloc: Foreign::at(f),
            release: Foreign::at(values::release(cx)?),
            free,
            used: usize::from(core.u8_at(f + os::USED)?),
            block: usize::from(core.u8_at(f + os::BLOCK)?),
        },
    );
    for &at in &calls {
        core.retarget(at + 1, alloc as extern "C" fn(usize, u32) -> usize)?;
    }
    for &at in &cleanups {
        core.retarget(at + 1, cleanup as extern "C" fn(usize))?;
    }
    Ok(core.report(&[calls, cleanups].concat()))
}
