//! Helpers for generating x86 code and executable memory for it ([`exec_code`], [`trampoline`]), calling BYOND's
//! functions from hooks ([`Foreign`]), run-time reads ([`live_u32`] / [`live_bytes`]) and the C allocator fn
//! types. Install code reads through [`crate::image::Image`].

use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[cfg(target_os = "linux")]
use crate::linux::mem as sys;
#[cfg(windows)]
use crate::windows::mem as sys;
use crate::{Result, bail};

/// The patched library's `malloc` and `realloc`.
pub type MallocFn = unsafe extern "C" fn(usize) -> *mut c_void;
pub type ReallocFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;

/// A foreign function of type `F` (an `extern "C"` fn pointer type): its address is found at install, hooks call
/// it through [`Foreign::get`]. Laid out as its address alone, so stubs can `jmp [static]` through it.
#[repr(transparent)]
pub struct Foreign<F> {
    addr: AtomicUsize,
    _f: PhantomData<F>,
}

impl<F: Copy> Foreign<F> {
    const FN_SIZED: () = assert!(size_of::<F>() == size_of::<usize>(), "F must be a fn pointer type");

    /// Not set yet.
    pub const fn new() -> Self {
        Self::at(0)
    }

    /// The function at `addr`.
    pub const fn at(addr: usize) -> Self {
        Self { addr: AtomicUsize::new(addr), _f: PhantomData }
    }

    /// Set its address (install time).
    pub fn set(&self, addr: impl CodeAddr) {
        self.addr.store(addr.code_addr(), Relaxed);
    }

    pub fn addr(&self) -> usize {
        self.addr.load(Relaxed)
    }

    /// The function, to call.
    ///
    /// # Safety
    /// The address set is a function with `F`'s signature and calling convention: install located and checked it.
    #[inline(always)]
    pub unsafe fn get(&self) -> F {
        let () = Self::FN_SIZED;
        let addr = self.addr();
        // SAFETY: `F` is a pointer-sized fn pointer type (checked above) and `addr` such a function (caller).
        unsafe { std::mem::transmute_copy::<usize, F>(&addr) }
    }
}

impl<F: Copy> Default for Foreign<F> {
    fn default() -> Self {
        Self::new()
    }
}

/// A code address: a `usize`, a fn pointer (a hook, a stub) or a naked function `as *const ()`.
pub trait CodeAddr {
    fn code_addr(self) -> usize;
}

impl CodeAddr for usize {
    #[inline(always)]
    fn code_addr(self) -> usize {
        self
    }
}

impl CodeAddr for *const () {
    #[inline(always)]
    fn code_addr(self) -> usize {
        self as usize
    }
}

macro_rules! code_addr_fns {
    ($($abi:literal),*) => { $(
        code_addr_fns!(@arity $abi;);
        code_addr_fns!(@arity $abi; A);
        code_addr_fns!(@arity $abi; A, B);
        code_addr_fns!(@arity $abi; A, B, C);
        code_addr_fns!(@arity $abi; A, B, C, D);
        code_addr_fns!(@arity $abi; A, B, C, D, E);
        code_addr_fns!(@arity $abi; A, B, C, D, E, G);
        code_addr_fns!(@arity $abi; A, B, C, D, E, G, H);
    )* };
    (@arity $abi:literal; $($a:ident),*) => {
        impl<R, $($a),*> CodeAddr for extern $abi fn($($a),*) -> R {
            #[inline(always)]
            fn code_addr(self) -> usize {
                self as usize
            }
        }
        impl<R, $($a),*> CodeAddr for unsafe extern $abi fn($($a),*) -> R {
            #[inline(always)]
            fn code_addr(self) -> usize {
                self as usize
            }
        }
    };
}
code_addr_fns!("C", "system", "thiscall", "fastcall");

/// `addr..addr+len` as it is now: BYOND's data, or code as features patched it.
///
/// # Safety
/// `addr..addr+len` must be mapped and readable.
#[inline(always)]
pub unsafe fn live_bytes<'a>(addr: usize, len: usize) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(addr as *const u8, len) }
}

/// The dword at `addr` as it is now.
///
/// # Safety
/// `addr` must be mapped and readable.
#[inline(always)]
pub unsafe fn live_u32(addr: usize) -> u32 {
    unsafe { (addr as *const u32).read_unaligned() }
}

/// `E8`/`E9` + rel32 from `at` to `target`, padded with NOPs to `len`.
pub fn branch(op: u8, at: usize, target: usize, len: usize) -> Vec<u8> {
    assert!(len >= 5, "branch: len {len} < 5");
    let mut v = vec![op];
    v.extend_from_slice(&(target.wrapping_sub(at + 5) as u32).to_le_bytes());
    v.resize(len, 0x90);
    v
}

/// `code` followed by NOPs up to `N` bytes, at compile time: new code for an in-place rewrite of an `N`-byte range.
pub const fn pad<const N: usize>(code: &[u8]) -> [u8; N] {
    let mut out = [0x90; N];
    out.split_at_mut(code.len()).0.copy_from_slice(code);
    out
}

/// Allocate `len` bytes of executable memory filled by `build(address)`. A wrong-length `build` is an error (the
/// patch turns itself off), not a crash. While a feature installs, freed if it (or its part) doesn't.
pub fn exec_code(len: usize, build: impl FnOnce(usize) -> Vec<u8>) -> Result<usize> {
    let p = sys::alloc_exec(len)?;
    let v = build(p);
    if v.len() != len {
        // SAFETY: `p` is `len` fresh bytes nothing uses yet.
        unsafe { sys::free_exec(p, len) };
        bail!("exec_code: built {} bytes, expected {len}", v.len());
    }
    // SAFETY: `p` is `len` fresh writable bytes nothing else uses until this returns; freed only on failure.
    unsafe {
        std::ptr::copy_nonoverlapping(v.as_ptr(), p as *mut u8, len);
        if let Err(e) = sys::seal_exec(p, len) {
            sys::free_exec(p, len);
            return Err(e);
        }
    }
    // SAFETY: runs only if the feature that wanted it doesn't install: nothing runs or points into it.
    crate::ledger::on_abort(Box::new(move || unsafe { sys::free_exec(p, len) }));
    Ok(p)
}

/// Executable copy of position-independent `code` followed by `jmp back`.
pub fn trampoline(code: &[u8], back: usize) -> Result<usize> {
    exec_code(code.len() + 5, |p| {
        let mut v = code.to_vec();
        v.extend(branch(0xE9, p + code.len(), back, 5));
        v
    })
}

/// IDA-style bytes of a little-endian u32 (an absolute address as it appears in code).
pub fn hx(v: usize) -> String {
    (v as u32).to_le_bytes().map(|b| format!("{b:02X}")).join(" ")
}
