//! `overloadcatch` (forum report 2992875): an error in an operator overload (`CRASH`, `throw`, a runtime error) is
//! caught by a `try` around the operator, instead of going to `world/Error` while the operator's statement goes on
//! with a null result.
//!
//! BYOND calls overloads like `set waitfor = 0` procs, and its search for the `try` that catches an error stops at
//! such a call. While an overload call runs, the fix lets the search go on past it to the caller: the overload's
//! proc then ends as any proc does whose caller catches its error, and once the call returns the error is raised
//! as if the call had thrown it, so the code around every call unwinds as it does for BYOND's own errors there.
//! `"[A]"` (`operator""`) keeps BYOND's behavior, as in BYOND's own fix (516.1690).
//!
//! The running calls are kept by stack position: a call BYOND left by unwinding is dropped as soon as a later call
//! or search sees the stack above it, so the search only goes past calls that are still running. Overloads that
//! sleep return to their caller at once as before, and an error with no `try` past the call still goes to
//! `world/Error`.

use crate::base::code::{self, Foreign};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::{Installed, Owned};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static OVERLOADCATCH: Feature = Feature::fix(
    Hosts::SERVER,
    "overloadcatch",
    "a try around an operator catches its overload's errors",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
)
.fixed_in(516_1690, "https://www.byond.com/forum/post/2992875");

/// A proc frame's call info and its caller's frame; the call info's flags, and the flag of a call made like
/// `set waitfor = 0`.
const CALL: usize = 0;
const CALLER: usize = 4;
const FLAGS: usize = 4;
const NO_WAIT: u8 = 0x10;

/// What install found: the running proc's frame (a global), a frame's `try` (its innermost handler), the flag a
/// proc's end sets when its caller is to catch its error, and BYOND's release of a value (cdecl: type, data).
struct Found {
    running: usize,
    try_at: usize,
    pending: usize,
    release: Foreign<unsafe extern "C" fn(u32, u32)>,
}

static FOUND: Installed<Found> = Installed::new();

/// The opcodes' overload calls running, innermost last: the calling proc's frame and the stack position of the
/// call.
static CALLS: Owned<Vec<(usize, usize)>> = Owned::new();

/// Drop the calls BYOND left by unwinding: those below `sp`, a stack position deeper than every running call.
fn drop_left(calls: &mut Vec<(usize, usize)>, sp: usize) {
    while calls.last().is_some_and(|&(_, at)| at <= sp) {
        calls.pop();
    }
}

/// An opcode is about to call an overload, at stack position `sp`.
extern "C" fn enter(sp: usize) {
    let Some(calls) = CALLS.get() else { return };
    drop_left(calls, sp);
    // SAFETY: the running proc's frame global, found at install.
    calls.push((unsafe { code::read::<usize>(FOUND.get().running) }, sp));
}

/// The overload call at `sp` returned `(ty, data)`. Whether its proc ended with an error that its caller's `try`
/// is to catch; if so the result is released, for the opcode to pass the error on instead of using it.
extern "C" fn leave(sp: usize, ty: u32, data: u32) -> bool {
    let Some(calls) = CALLS.get() else { return false };
    drop_left(calls, sp - 1);
    if calls.last().is_some_and(|&(_, at)| at == sp) {
        calls.pop();
    }
    let found = FOUND.get();
    // SAFETY: the flag global, found at install; BYOND's release of the value the call returned. The flag is off
    // while the release runs, so a proc it runs (a `Del()`) doesn't take the error as its own.
    unsafe {
        if code::read::<u8>(found.pending) == 0 {
            return false;
        }
        code::write::<u8>(found.pending, 0);
        found.release.get()(ty, data);
        code::write::<u8>(found.pending, 1);
    }
    true
}

/// BYOND's search for the `try` that catches an error raised in `frame`: the innermost handler of `frame`'s callers
/// (0: none), stopping at a `waitfor = 0` call unless it is a running overload call of the caller.
extern "C" fn find_try_from(frame: usize) -> usize {
    let found = FOUND.get();
    let here = 0u8;
    let calls = CALLS.get().map(|calls| {
        drop_left(calls, &here as *const u8 as usize);
        &calls[..]
    });
    let mut calls = calls.unwrap_or(&[]);
    let mut frame = frame;
    // SAFETY: BYOND's frames: each caller is live while the frames it called run.
    unsafe {
        loop {
            let caller = code::read::<usize>(frame + CALLER);
            if caller == 0 {
                return 0;
            }
            if code::read::<u8>(code::read::<usize>(frame + CALL) + FLAGS) & NO_WAIT != 0 {
                match calls.split_last() {
                    Some((&(by, _), rest)) if by == caller => calls = rest,
                    _ => return 0,
                }
            }
            frame = caller;
            let handler = code::read::<usize>(frame + found.try_at);
            if handler != 0 {
                return handler;
            }
        }
    }
}

/// [`find_try_from`] the running proc.
extern "C" fn find_try() -> usize {
    // SAFETY: the running proc's frame global, found at install.
    find_try_from(unsafe { code::read::<usize>(FOUND.get().running) })
}
