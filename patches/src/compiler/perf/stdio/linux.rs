//! `stdio` and `ftell`, Linux: libbyond's `getc`, `fgetc`, `fputc`, and `putc` calls through glibc's unlocked fast
//! paths, and its `ftell` calls answered from glibc's stream state.
//!
//! glibc answers `ftell` with an `lseek`. A plainly writing stream's position is `_offset` plus the buffered bytes,
//! once a real `ftell` has set `_offset` as a seek would; glibc keeps it current from then on.

use std::ffi::{c_int, c_long};

use super::verify;
use crate::base::driver::Counter;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::linux::glibc::{
    File, IO_CURRENTLY_PUTTING, IO_IS_APPENDING, fgetc, fputc, fputc_unlocked, ftell, getc, getc_unlocked, is_standard,
    putc,
};
use crate::base::{Result, ensure};

type GetcFn = unsafe extern "C" fn(*mut File) -> c_int;
type FputcFn = unsafe extern "C" fn(c_int, *mut File) -> c_int;
type FtellFn = unsafe extern "C" fn(*mut File) -> c_long;

/// `ftell` calls answered from the stream's state, and those left to glibc.
static FAST: Counter = Counter::new();
static SLOW: Counter = Counter::new();

/// Replaces `getc` and `fgetc`: glibc's `getc_unlocked` macro, except on a standard stream.
///
/// # Safety
/// `fp` is a live glibc stream (BYOND passes it).
unsafe extern "C" fn fast_getc(fp: *mut File) -> c_int {
    // SAFETY: a live stream (caller), with its unread bytes at read_ptr..read_end.
    unsafe {
        if is_standard(fp) {
            return getc(fp);
        }
        let f = &mut *fp;
        if f.read_ptr < f.read_end {
            let c = *f.read_ptr;
            f.read_ptr = f.read_ptr.add(1);
            return c_int::from(c);
        }
        getc_unlocked(fp)
    }
}

/// Replaces `fputc` and `putc`: glibc's `putc_unlocked` macro, except on a standard stream.
///
/// # Safety
/// `fp` is a live glibc stream (BYOND passes it).
unsafe extern "C" fn fast_fputc(c: c_int, fp: *mut File) -> c_int {
    // SAFETY: a live stream (caller), with room at write_ptr..write_end.
    unsafe {
        if is_standard(fp) {
            return fputc(c, fp);
        }
        let f = &mut *fp;
        if f.write_ptr < f.write_end {
            *f.write_ptr = c as u8;
            f.write_ptr = f.write_ptr.add(1);
            return c_int::from(c as u8);
        }
        fputc_unlocked(c, fp)
    }
}

/// Replaces `ftell`: a plainly writing stream's position from its state, else glibc's answer.
///
/// # Safety
/// `fp` is a live glibc stream (BYOND passes it).
unsafe extern "C" fn fast_ftell(fp: *mut File) -> c_long {
    // SAFETY: a live stream (caller); a writing one buffers write_base..write_ptr.
    unsafe {
        let f = &mut *fp;
        let writing = !is_standard(fp)
            && f.flags & (IO_CURRENTLY_PUTTING | IO_IS_APPENDING) == IO_CURRENTLY_PUTTING
            && f.read_end == f.write_base;
        if !writing {
            SLOW.bump();
            return ftell(fp);
        }
        let buffered = f.write_ptr.offset_from(f.write_base) as i64;
        if f.offset < 0 {
            // Opened and written, never sought: store what a seek would have.
            SLOW.bump();
            let real = ftell(fp);
            if real >= 0 {
                f.offset = i64::from(real) - buffered;
            }
            return real;
        }
        let Ok(pos) = c_long::try_from(f.offset + buffered) else {
            SLOW.bump();
            return ftell(fp);
        };
        FAST.bump();
        if crate::base::driver::verify() {
            verify::compare(pos, ftell(fp));
        }
        pos
    }
}

/// Every call of `name` (glibc's `libc_fn`) goes to `ours`.
fn redirect(core: &Image, name: &str, libc_fn: usize, ours: usize) -> Result<String> {
    let sites = core.import_calls(name, libc_fn)?;
    ensure!(!sites.is_empty(), "no {name} call sites");
    for &site in &sites {
        core.retarget(site + 1, ours)?;
    }
    Ok(format!("{name} {}", sites.len()))
}

/// Each function's calls are a part.
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (read, write) = (fast_getc as GetcFn as usize, fast_fputc as FputcFn as usize);
    let mut parts = cx.parts();
    for (name, libc_fn, ours) in [
        ("getc", getc as GetcFn as usize, read),
        ("fgetc", fgetc as GetcFn as usize, read),
        ("fputc", fputc as FputcFn as usize, write),
        ("putc", putc as FputcFn as usize, write),
    ] {
        parts.part(|| redirect(core, name, libc_fn, ours));
    }
    parts.finish()
}

pub(super) fn install_ftell(cx: &Ctx) -> Result<String> {
    redirect(cx.core(), "ftell", ftell as FtellFn as usize, fast_ftell as FtellFn as usize)
}

pub(super) fn ftell_stats() {
    eprintln!(
        "dmfast: ftell fast={FAST} slow={SLOW} verify={} mismatches={}",
        crate::base::driver::verify(),
        verify::MISMATCHES
    );
}
