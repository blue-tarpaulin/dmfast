//! Code for hooks: calling BYOND's functions ([`Foreign`]), building x86 stubs in executable memory ([`Stub`],
//! [`trampoline`]), bytes for code rewritten in place ([`branch`], [`pad`]), run-time reads of BYOND's memory
//! ([`live_u32`]), and the C allocator fn types. Install code reads BYOND's code through [`crate::image::Image`].

use std::ffi::c_void;

mod bytes;
mod foreign;
mod stub;

pub use bytes::{branch, hx, pad};
pub use foreign::{CodeAddr, Foreign};
pub use stub::{Cond, Stub, trampoline};

/// The patched library's `malloc`.
pub type MallocFn = unsafe extern "C" fn(usize) -> *mut c_void;
/// The patched library's `realloc`.
pub type ReallocFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;

/// The dword at `addr` as it is now: for hooks at run time (install code reads through
/// [`crate::image::Image`], whose `live_u32` is checked and returns `Result`).
///
/// # Safety
/// `addr` must be mapped and readable.
#[inline(always)]
pub unsafe fn live_u32(addr: usize) -> u32 {
    unsafe { (addr as *const u32).read_unaligned() }
}
