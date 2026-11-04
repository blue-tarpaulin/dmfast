//! glibc's i386 `FILE` layout (features `stdio`, `macrospace`, `dmbread`), and the stdio functions those features
//! call or redirect: a crate may declare a C function only once.

use std::ffi::{c_int, c_long};

/// glibc's i386 `FILE` up to `_offset` (stable ABI: the stdio macros compile these offsets in).
#[repr(C)]
pub struct File {
    pub flags: c_int,
    pub read_ptr: *mut u8,
    pub read_end: *mut u8,
    pub read_base: *mut u8,
    pub write_base: *mut u8,
    pub write_ptr: *mut u8,
    pub write_end: *mut u8,
    _rest: [u32; 12],
    pub offset: i64,
}
const _: () = assert!(std::mem::offset_of!(File, offset) == 0x4C);

/// `_IO_CURRENTLY_PUTTING`: the stream's buffer holds writes.
pub const IO_CURRENTLY_PUTTING: c_int = 0x800;
/// `_IO_IS_APPENDING`: opened in append mode.
pub const IO_IS_APPENDING: c_int = 0x1000;

unsafe extern "C" {
    static stdin: *mut File;
    static stdout: *mut File;
    static stderr: *mut File;
    pub fn getc(fp: *mut File) -> c_int;
    pub fn fgetc(fp: *mut File) -> c_int;
    pub fn getc_unlocked(fp: *mut File) -> c_int;
    pub fn ungetc(c: c_int, fp: *mut File) -> c_int;
    pub fn fputc(c: c_int, fp: *mut File) -> c_int;
    pub fn putc(c: c_int, fp: *mut File) -> c_int;
    pub fn fputc_unlocked(c: c_int, fp: *mut File) -> c_int;
    pub fn ftell(fp: *mut File) -> c_long;
}

/// Whether `fp` is stdin, stdout, or stderr, which other threads use too.
#[inline(always)]
pub fn is_standard(fp: *mut File) -> bool {
    // SAFETY: glibc's standard stream pointers, only compared.
    unsafe { fp == stdin || fp == stdout || fp == stderr }
}
