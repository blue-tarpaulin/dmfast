//! dmfast's allocator: mimalloc, except requests from `SPLIT` up to `LARGE` bytes, which go to glibc.
//!
//! mimalloc avoids glibc's per-call atomic locks, but its size classes waste memory on BYOND's many mid-sized
//! blocks, which glibc packs. Large blocks stay with mimalloc as glibc reallocs the compiler's big arrays slower;
//! a glibc block grown past `LARGE` moves to mimalloc. The exports below interpose glibc's for the process
//! (preloaded library); mimalloc's `override` mode isn't used as a Rust cdylib keeps static C symbols local.
//! Allocation addresses never reach the .dmb. Blocks are told apart by mimalloc's page map; anything outside it
//! (including blocks glibc handed out before our first call) is glibc's. `DMFAST_DISABLE=mimalloc` sends every
//! call to glibc, chosen at the first call and never changed.

#![allow(clippy::missing_safety_doc)] // the exports below are C's allocator API

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use libc::{__errno_location, EINVAL, ENOMEM};
use libmimalloc_sys as mi;

use crate::code::Foreign;
use crate::linux::PAGE;

// glibc's allocator under its internal names (the public ones resolve to the exports below).
unsafe extern "C" {
    fn __libc_malloc(size: usize) -> *mut c_void;
    fn __libc_calloc(count: usize, size: usize) -> *mut c_void;
    fn __libc_realloc(p: *mut c_void, size: usize) -> *mut c_void;
    fn __libc_free(p: *mut c_void);
    fn __libc_memalign(align: usize, size: usize) -> *mut c_void;
}

/// Requests from this many bytes up to [`LARGE`] go to glibc.
const SPLIT: usize = 4096;
/// Requests from this many bytes go back to mimalloc.
const LARGE: usize = 256 * 1024;
/// `mi_option_generic_collect` in the bundled mimalloc 3.3.2 (unnamed in libmimalloc-sys); recheck on upgrade.
const GENERIC_COLLECT: mi::mi_option_t = 34;

/// 0: not chosen yet, 1: split, 2: glibc only.
static MODE: AtomicU8 = AtomicU8::new(0);

/// The allocator is glibc's alone (`DMFAST_DISABLE=mimalloc`).
#[inline]
fn glibc() -> bool {
    match MODE.load(Relaxed) {
        1 => false,
        2 => true,
        _ => choose(),
    }
}

/// Whether a request of `size` bytes goes to mimalloc.
#[inline]
fn small(size: usize) -> bool {
    !(SPLIT..LARGE).contains(&size) && !glibc()
}

/// Whether a glibc block resized to `size` moves to mimalloc.
#[inline]
fn big(size: usize) -> bool {
    size >= LARGE && !glibc()
}

#[inline]
fn ours(p: *const c_void) -> bool {
    // SAFETY: mimalloc's page map query takes any pointer.
    !p.is_null() && !glibc() && unsafe { mi::mi_is_in_heap_region(p) }
}

/// Reads `DMFAST_DISABLE` without allocating; the first call comes before any thread starts.
#[cold]
fn choose() -> bool {
    // SAFETY: getenv before any thread exists (see above); a non-null result is a NUL-terminated string.
    let off = unsafe {
        let v = libc::getenv(c"DMFAST_DISABLE".as_ptr());
        !v.is_null()
            && std::ffi::CStr::from_ptr(v).to_bytes().split(|&b| b == b',').any(|k| k.trim_ascii() == b"mimalloc")
    };
    if !off {
        // Set up mimalloc before `ours` queries its page map; max the collect interval (env var still overrides).
        // SAFETY: mimalloc's API; setting an option's default before mimalloc reads it.
        unsafe {
            mi::mi_option_set_default(GENERIC_COLLECT, 1_000_000);
            mi::mi_free(mi::mi_malloc(1));
        }
    }
    MODE.store(if off { 2 } else { 1 }, Relaxed);
    off
}

/// How the allocator runs, for the stats line.
pub(crate) fn alloc_status() -> String {
    if glibc() {
        return "mimalloc off".into();
    }
    // SAFETY: no arguments; mimalloc is linked in.
    let version = unsafe { mi::mi_version() };
    crate::driver::mimalloc(version, &format!("glibc for {}-{} KB", SPLIT / 1024, LARGE / 1024))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    unsafe { if small(size) { mi::mi_malloc(size) } else { __libc_malloc(size) } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        unsafe { *__errno_location() = ENOMEM };
        return std::ptr::null_mut();
    };
    unsafe { if small(total) { mi::mi_calloc(count, size) } else { __libc_calloc(count, size) } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut c_void, size: usize) -> *mut c_void {
    unsafe {
        if p.is_null() {
            return malloc(size);
        }
        if !ours(p) {
            if !big(size) {
                // glibc blocks stay glibc's while mid-sized (or predate the library).
                return __libc_realloc(p, size);
            }
            let q = mi::mi_malloc(size);
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(p as *const u8, q as *mut u8, malloc_usable_size(p).min(size));
                __libc_free(p);
            }
            return q;
        }
        if small(size) {
            return mi::mi_realloc(p, size);
        }
        let q = __libc_malloc(size);
        if !q.is_null() {
            std::ptr::copy_nonoverlapping(p as *const u8, q as *mut u8, mi::mi_usable_size(p).min(size));
            mi::mi_free(p);
        }
        q
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reallocarray(p: *mut c_void, count: usize, size: usize) -> *mut c_void {
    match count.checked_mul(size) {
        Some(total) => unsafe { realloc(p, total) },
        None => unsafe {
            *__errno_location() = ENOMEM;
            std::ptr::null_mut()
        },
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut c_void) {
    // BYOND frees null often: skip the page-map lookup and glibc for it.
    if p.is_null() {
        return;
    }
    unsafe { if ours(p) { mi::mi_free(p) } else { __libc_free(p) } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cfree(p: *mut c_void) {
    unsafe { free(p) }
}

/// glibc's `malloc_usable_size` (its only name is the one exported below), looked up on first use.
static GLIBC_USABLE: Foreign<unsafe extern "C" fn(*mut c_void) -> usize> = Foreign::new();

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(p: *mut c_void) -> usize {
    unsafe {
        if ours(p) {
            return mi::mi_usable_size(p);
        }
        if GLIBC_USABLE.addr() == 0 {
            GLIBC_USABLE.set(libc::dlsym(libc::RTLD_NEXT, c"malloc_usable_size".as_ptr()) as usize);
        }
        if GLIBC_USABLE.addr() == 0 { 0 } else { GLIBC_USABLE.get()(p) }
    }
}

unsafe fn aligned(align: usize, size: usize) -> *mut c_void {
    unsafe { if small(size) { mi::mi_malloc_aligned(size, align) } else { __libc_memalign(align, size) } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memalign(align: usize, size: usize) -> *mut c_void {
    unsafe { aligned(align, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut c_void {
    unsafe { aligned(align, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valloc(size: usize) -> *mut c_void {
    unsafe { aligned(PAGE, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pvalloc(size: usize) -> *mut c_void {
    let rounded = size.checked_next_multiple_of(PAGE).unwrap_or(usize::MAX);
    unsafe { aligned(PAGE, rounded) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if !align.is_power_of_two() || !align.is_multiple_of(size_of::<*mut c_void>()) {
        return EINVAL;
    }
    let p = unsafe { aligned(align, size) };
    if p.is_null() && size != 0 {
        return ENOMEM;
    }
    unsafe { *out = p };
    0
}
