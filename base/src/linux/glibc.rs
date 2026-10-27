//! glibc's i386 `FILE` layout, shared by the stream fast paths (features `stdio`, `macrospace`, `dmbread`).

use std::ffi::c_int;

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
