//! What every DreamSeeker patch compiles with (`author::compile` makes it the crate's `prelude` module, before the
//! patch): the built-in addresses a patch jumps to or uses, `comN` to call Direct3D 9 methods, `text!` for its
//! strings, and what `no_std` code needs (a panic handler and `memcpy`, `memmove`, `memset`, `memcmp`). Use it with
//! `use crate::prelude::*;`.

// A patch uses only some of it; `comN` declare `dmf_com` with ten arities.
#![allow(dead_code, unused_macros, clashing_extern_declarations)]

use core::arch::asm;
use core::ffi::c_void;

/// A pointer, such as a COM object.
pub type P = *mut c_void;

unsafe extern "C" {
    /// The instruction after the site.
    pub fn dmf_resume();
    /// The site's original instructions, then the instruction after the site. A patch that calls it as a function
    /// declares its own typed `extern "thiscall"` (or other) with `#[link_name = "dmf_original"]`.
    pub fn dmf_original();
    /// Jump here with `eax` = a vtable slot and the stack as the method expects (`[ret][this][args..]`) to run
    /// method `slot` of a Direct3D 9 object (checked: `seeker/src/com.rs`).
    pub fn dmf_com_tail();
    /// The patch's zero-filled data (`data` in its spec), after its zero-initialised statics.
    pub static dmf_data: u8;
}

/// Panics stop DreamSeeker at once.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { asm!("ud2", options(noreturn, nomem, nostack)) }
}

// `esi` can't be an asm operand on i686, so it goes through a register and back. The signatures are the C library's.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe {
        asm!(
            "xchg esi, {s}",
            "rep movsb",
            "mov esi, {s}",
            s = inout(reg) src => _,
            inout("edi") dst => _,
            inout("ecx") n => _,
            options(nostack, preserves_flags),
        );
    }
    dst
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if (dst as usize).wrapping_sub(src as usize) >= n {
        // `dst` before `src`, or after its end: forwards.
        return unsafe { memcpy(dst, src, n) };
    }
    unsafe {
        asm!(
            "xchg esi, {s}",
            "std",
            "rep movsb",
            "cld",
            "mov esi, {s}",
            s = inout(reg) src.cast::<u8>().wrapping_add(n - 1) => _,
            inout("edi") dst.cast::<u8>().wrapping_add(n - 1) => _,
            inout("ecx") n => _,
            options(nostack),
        );
    }
    dst
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dst: *mut c_void, c: i32, n: usize) -> *mut c_void {
    unsafe {
        asm!(
            "rep stosb",
            inout("eax") c => _,
            inout("edi") dst => _,
            inout("ecx") n => _,
            options(nostack, preserves_flags),
        );
    }
    dst
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    let out: i32;
    unsafe {
        asm!(
            "xchg esi, {s}",
            "xor eax, eax",
            "repe cmpsb",
            "je 2f",
            "movzx eax, byte ptr [esi - 1]",
            "movzx edx, byte ptr [edi - 1]",
            "sub eax, edx",
            "2:",
            "mov esi, {s}",
            s = inout(reg) a => _,
            inout("edi") b => _,
            inout("ecx") n => _,
            out("eax") out,
            out("edx") _,
            options(nostack, readonly),
        );
    }
    out
}

/// `comN`: calls method `slot` of COM object `obj` with `N` arguments through the checked Direct3D helper, which
/// allows only Direct3D 9's methods; returns its `eax` (an HRESULT, or a count for AddRef/Release).
macro_rules! com {
    ($($name:ident $link:ident ($($a:ident),*);)*) => {$(
        unsafe extern "system" {
            #[link_name = "dmf_com"]
            fn $link(slot: usize, obj: P $(, $a: usize)*) -> i32;
        }

        #[inline(always)]
        #[allow(clippy::too_many_arguments)] // a COM method's arguments
        pub unsafe fn $name(obj: P, slot: usize $(, $a: usize)*) -> i32 {
            unsafe { $link(slot, obj $(, $a)*) }
        }
    )*};
}

com! {
    com0 dmf_com0();
    com1 dmf_com1(a1);
    com2 dmf_com2(a1, a2);
    com3 dmf_com3(a1, a2, a3);
    com4 dmf_com4(a1, a2, a3, a4);
    com5 dmf_com5(a1, a2, a3, a4, a5);
    com6 dmf_com6(a1, a2, a3, a4, a5, a6);
    com7 dmf_com7(a1, a2, a3, a4, a5, a6, a7);
    com8 dmf_com8(a1, a2, a3, a4, a5, a6, a7, a8);
    com9 dmf_com9(a1, a2, a3, a4, a5, a6, a7, a8, a9);
}

/// `text!(NAME = "...")`: a NUL-terminated string constant named `NAME` (`sym NAME` in asm). The text may be any
/// constant `&str`, such as `concat!("... dmfast ", env!("DMFAST_VERSION"))`.
macro_rules! text {
    ($name:ident = $text:expr) => {
        #[unsafe(no_mangle)]
        pub static $name: [u8; $text.len() + 1] = $crate::prelude::with_nul($text);
    };
}

/// `text` and a NUL, as `N` (its length plus one) bytes.
pub const fn with_nul<const N: usize>(text: &str) -> [u8; N] {
    let (mut out, text) = ([0; N], text.as_bytes());
    let mut i = 0;
    while i < text.len() {
        out[i] = text[i];
        i += 1;
    }
    out
}
