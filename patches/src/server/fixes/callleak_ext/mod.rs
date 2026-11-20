//! `call_ext()` implementation for the combined `callleak` fix: `arglist()` no longer leaks a reference to every arg.
//!
//! Both call_ext() opcodes that take an `arglist()` copy the list's items into a buffer, referencing each, but the
//! buffer's cleanup does nothing, so every item stays referenced for good. A long list's allocated block is leaked too.
//! The cleanup releases the items and frees the block.
//!
//! Safe: the cleanup runs when the call is done with the buffer, and the items were referenced for it alone.

use crate::base::code::{self, Foreign};
use crate::base::feature::Ctx;
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

type AllocFn = unsafe extern "C" fn(usize, u32) -> usize;

struct Found {
    alloc: Foreign<AllocFn>,
    release: Foreign<ReleaseFn>,
    free: Foreign<unsafe extern "C" fn(usize)>,
    used: usize,
    block: usize,
}

static FOUND: Installed<Found> = Installed::new();

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

fn shipped_u32(core: &Image, at: usize) -> Result<u32> {
    let b = core.haystack(at, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn shipped_target(core: &Image, call: usize) -> Result<usize> {
    Ok((call + 5).wrapping_add(shipped_u32(core, call + 1)? as i32 as usize))
}

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

pub(super) fn install(cx: &Ctx) -> Result<String> {
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
