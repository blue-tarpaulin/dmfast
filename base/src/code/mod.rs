//! Code for hooks: calling BYOND's functions ([`Foreign`]) and jumping to its code ([`Addr`]), building x86 stubs
//! ([`Stub`], [`trampoline`]), bytes for code rewritten in place ([`branch`], [`pad`]), run-time reads and writes of
//! BYOND's memory ([`read`], [`write`], [`live_u32`]), and the C allocator fn types. Install code reads BYOND's code
//! through [`crate::image::Image`].

use std::ffi::c_void;

mod bytes;
mod foreign;
mod stub;

pub use bytes::{branch, hx, pad};
pub use foreign::{Addr, CodeAddr, Foreign};
pub use stub::{Cond, Label, Stub, Target, trampoline};

/// The patched library's `malloc`.
pub type MallocFn = unsafe extern "C" fn(usize) -> *mut c_void;
/// The patched library's `realloc`.
pub type ReallocFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;

/// The `T` at `addr` as it is now, unaligned: a field of BYOND's data, for hooks at run time (install code reads
/// through [`crate::image::Image`]). Name the type: `read::<u16>(node + 6)`.
///
/// # Safety
/// `addr..addr+size_of::<T>()` is mapped and readable, and holds a valid `T`.
#[inline(always)]
pub unsafe fn read<T: Copy>(addr: usize) -> T {
    unsafe { (addr as *const T).read_unaligned() }
}

/// Store `v` at `addr`, unaligned: a field of BYOND's data, for hooks at run time.
///
/// # Safety
/// `addr..addr+size_of::<T>()` is mapped, writable, and nothing else uses it meanwhile.
#[inline(always)]
pub unsafe fn write<T: Copy>(addr: usize, v: T) {
    unsafe { (addr as *mut T).write_unaligned(v) }
}

/// [`read::<u32>`](read), the most common read (a pointer, a count, an id).
///
/// # Safety
/// As [`read`].
#[inline(always)]
pub unsafe fn live_u32(addr: usize) -> u32 {
    unsafe { read(addr) }
}
