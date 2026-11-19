//! `sha1leak`: `sha1()` no longer leaks memory.
//!
//! SHA-1's finish returns its 20-byte digest in a block the caller frees, as BYOND's other user of it does, but
//! `sha1()` only writes the digest out in hex. On a file, it also reads through a 4 KB buffer it never frees. The fix
//! frees the digest after the hex and the buffer when the file is closed.
//!
//! Safe: both blocks are freed after their last use.

use crate::base::code::Foreign;
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static SHA1LEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "sha1leak",
    "sha1() frees its digest and file buffer",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// `StringEditor::AddHex` (the editor, the bytes, their count).
#[cfg(windows)]
type AddHex = unsafe extern "thiscall" fn(usize, usize, i32);
#[cfg(target_os = "linux")]
type AddHex = unsafe extern "C" fn(usize, usize, i32);

/// BYOND's `AddHex`, and the C runtime's `fclose` and `free` (Windows: BYOND's, not Rust's).
struct Found {
    add_hex: Foreign<AddHex>,
    fclose: Foreign<unsafe extern "C" fn(usize) -> i32>,
    free: Foreign<unsafe extern "C" fn(usize)>,
}

static FOUND: Installed<Found> = Installed::new();

/// In place of `sha1()`'s hex of the digest: the hex, then the digest's free.
#[cfg(windows)]
unsafe extern "thiscall" fn add_hex(editor: usize, digest: usize, len: i32) {
    // SAFETY: BYOND's call, passed on.
    unsafe { hex(editor, digest, len) }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn add_hex(editor: usize, digest: usize, len: i32) {
    // SAFETY: BYOND's call, passed on.
    unsafe { hex(editor, digest, len) }
}

/// # Safety
/// BYOND's `AddHex` arguments, with the digest SHA-1's finish returned.
unsafe fn hex(editor: usize, digest: usize, len: i32) {
    let found = FOUND.get();
    // SAFETY: BYOND's hex, then the free of the block SHA-1's finish allocated, which nothing else holds.
    unsafe {
        found.add_hex.get()(editor, digest, len);
        found.free.get()(digest);
    }
}

/// In place of the file branch's `fclose`: the close, then the free of the read buffer.
extern "C" fn close(file: usize, buffer: usize) {
    let found = FOUND.get();
    // SAFETY: BYOND's open file and its malloc'd read buffer, both done with.
    unsafe {
        found.fclose.get()(file);
        found.free.get()(buffer);
    }
}

/// `close_stub`, in place of the file branch's `call fclose` (cdecl, the file), with the buffer in `$buffer`.
/// 16-byte stack alignment at the call.
macro_rules! close_stub {
    ($buffer:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn close_stub() {
            std::arch::naked_asm!(
                "sub esp, 4",
                concat!("push ", $buffer),
                "push dword ptr [esp + 12]",
                "call {close}",
                "add esp, 12",
                "ret",
                close = sym super::close,
            )
        }
    };
}
use close_stub;
