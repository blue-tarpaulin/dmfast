//! `callref` (forum report 2991514): a `call(callee)(args)` that fails no longer leaks a reference to every arg.
//!
//! Both call() opcodes (a list of args, and a single `arglist()`, which named args also compile to) abandon the call
//! on "bad proc" or "call_ext() must be used ..." without releasing the args. Before each of those failure reports,
//! the fix releases the args and nulls their stack slots.

use crate::base::Result;
use crate::base::code::{CodeAddr, live_u32, trampoline};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static CALLREF: Feature = Feature::fix(
    Hosts::SERVER,
    "callref",
    "a failed call() releases its args",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// A stack slot: a value's type and data.
type Slot = [u32; 2];

/// The call's args: `count` slots after the callee's on the interpreter's stack. `state` is the interpreter state,
/// with the stack's address at `base` and the callee's position on it at `pos`.
///
/// # Safety
/// `state` is the interpreter state and `base` and `pos` its fields' offsets.
unsafe fn args(state: usize, base: usize, pos: usize, count: u32) -> impl Iterator<Item = *mut Slot> {
    // SAFETY: the state's fields (caller).
    let (stack, callee) = unsafe { (live_u32(state + base) as usize, ((state + pos) as *const u16).read_unaligned()) };
    let first = stack + 8 * usize::from(callee) + 8;
    (0..count as usize).map(move |i| (first + 8 * i) as *mut Slot)
}

/// Make the failure report at `site` call `release` first: `push` puts its arguments on the stack (`pushed` bytes,
/// popped after the call), then the report's first `len` bytes (position-independent) run as before.
fn release_first(core: &Image, site: usize, len: usize, push: &[u8], pushed: u8, release: impl CodeAddr) -> Result<()> {
    let mut v = push.to_vec();
    v.push(0xB8); // mov eax,release
    v.extend((release.code_addr() as u32).to_le_bytes());
    v.extend([0xFF, 0xD0]); // call eax
    v.extend([0x83, 0xC4, pushed]); // add esp,pushed
    v.extend(core.bytes(site, len)?);
    core.jump(site, len, trampoline(&v, site + len)?)
}
