//! [`Foreign`]: a typed pointer to BYOND's function, and [`CodeAddr`]: anything that is a code address.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

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
        Self { addr: AtomicUsize::new(0), _f: PhantomData }
    }

    /// The function at `addr`.
    pub fn at(addr: impl CodeAddr) -> Self {
        Self { addr: AtomicUsize::new(addr.code_addr()), _f: PhantomData }
    }

    /// Set its address. Not undone if the install fails, which is harmless for a function only a hook calls; for
    /// state hooks rely on, use [`crate::sync::Installed`].
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
