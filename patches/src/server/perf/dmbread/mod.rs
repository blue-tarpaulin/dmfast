//! `dmbread`: faster .dmb loading: whole-value reads from the stream buffer, and `ftell` without a syscall.
//!
//! BYOND reads each value with one `fgetc`/`getc` call per byte. Each reader here replaces such a helper with one
//! [`Stream::read`] and returns the same, also at end of file (a byte that can't be read counts as -1). The helpers
//! also read savefiles during play. How each platform answers `ftell` from what it already knows, and why that's
//! exact, is in its file; `DMFAST_VERIFY=1` also asks the CRT and counts disagreements.
//!
//! Dependency guard (`deps`, Windows): the string reader and its only caller, the string table load.

use crate::base::code::{Foreign, MallocFn, read, write};
use crate::base::feature::{Feature, Hosts, install};
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static DMBREAD: Feature =
    Feature::perf(Hosts::SERVER, "dmbread", ".dmb reader", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);

/// [`read_value`]: `(fp) -> value`.
type ReadFn = unsafe extern "C" fn(usize) -> u32;
/// [`read_u16_keyed`]: `(fp, key) -> value`.
type KeyedFn = unsafe extern "C" fn(usize, u32) -> u32;
/// [`read_keyed`]: `(dst, n, fp, key)`.
type BytesFn = unsafe extern "C" fn(*mut u8, u32, usize, u32);
/// [`read_id_array`]: `(record, fp)`.
type IdsFn = unsafe extern "C" fn(usize, usize);

/// An id array record: the count (u16), and the `malloc`ed array of ids (a dword each).
const RECORD_COUNT: usize = 0;
const RECORD_IDS: usize = 8;

/// What [`read_id_array`] needs: BYOND's `malloc`, and the .dmb format flag's address (nonzero: 32-bit ids).
struct IdArray {
    malloc: Foreign<MallocFn>,
    wide: usize,
}

static ID_ARRAY: Installed<IdArray> = Installed::new();

/// A platform's stream access.
trait Stream {
    /// `fread(buf, 1, n, fp)` on BYOND's `FILE*`.
    ///
    /// # Safety
    /// `fp` is a readable stream and `buf` holds `n` bytes.
    unsafe fn read(fp: usize, buf: *mut u8, n: usize) -> usize;
}

/// Bytes as `fgetc` returns them one by one (`byte(i)`: 0..=255, or -1 past what could be read), combined as BYOND
/// does: `b0 | b1<<8 | ...`, so a -1 sets every bit above it.
#[inline(always)]
fn combine(n: usize, byte: impl Fn(usize) -> i32) -> u32 {
    (0..n).fold(0i32, |v, i| v | byte(i).wrapping_shl(8 * i as u32)) as u32
}

/// Replaces the 16- and 32-bit readers (`N` bytes): `b0 | b1<<8 | ...`.
///
/// # Safety
/// `fp` is a readable stream (BYOND passes it).
unsafe extern "C" fn read_value<S: Stream, const N: usize>(fp: usize) -> u32 {
    let mut b = [0u8; N];
    // SAFETY: a readable stream (caller) into `N` bytes.
    let got = unsafe { S::read(fp, b.as_mut_ptr(), N) };
    combine(N, |i| if i < got { i32::from(b[i]) } else { -1 })
}

/// Replaces the keyed 16-bit reader: `(b0 | b1<<8) ^ key`.
///
/// # Safety
/// `fp` is a readable stream (BYOND passes it).
unsafe extern "C" fn read_u16_keyed<S: Stream>(fp: usize, key: u32) -> u32 {
    // SAFETY: a readable stream (caller).
    unsafe { read_value::<S, 2>(fp) ^ key }
}

/// Replaces the n-byte reader: `dst[i] = getc(fp) ^ key`, the key's low byte growing by 9 per byte. It returns
/// nothing: every caller ignores the original's result ([`result_unused`]).
///
/// # Safety
/// `dst` holds `n` bytes and `fp` is a readable stream (BYOND passes them).
unsafe extern "C" fn read_keyed<S: Stream>(dst: *mut u8, n: u32, fp: usize, key: u32) {
    if n == 0 {
        return;
    }
    // SAFETY: `n` bytes at `dst` and a readable stream (caller).
    unsafe {
        let n = n as usize;
        let got = S::read(fp, dst, n);
        let mut k = key as u8;
        for i in 0..n {
            // A byte getc couldn't read is -1, whose low byte is FFh.
            let b = if i < got { *dst.add(i) } else { 0xFF };
            *dst.add(i) = b ^ k;
            k = k.wrapping_add(9);
        }
    }
}

/// Replaces the id-array reader: a 16-bit count, a `malloc`ed array of `count` dwords (unchecked, as by BYOND), then
/// the ids: 32-bit if the format flag is set, else 16-bit.
///
/// # Safety
/// `record` is an id array record and `fp` a readable stream (BYOND passes them).
unsafe extern "C" fn read_id_array<S: Stream>(record: usize, fp: usize) {
    let found = ID_ARRAY.get();
    // SAFETY: a record and a readable stream (caller); BYOND's `malloc` and format flag (install) give `4n` bytes.
    unsafe {
        let n = read_value::<S, 2>(fp) as u16;
        write(record + RECORD_COUNT, n);
        let ids = found.malloc.get()(usize::from(n) * 4).cast::<u8>();
        write(record + RECORD_IDS, ids as u32);
        let n = usize::from(n);
        if n == 0 {
            return;
        }
        let w = if read::<u8>(found.wide) != 0 { 4 } else { 2 };
        // 16-bit ids are read into the upper half of the array and widened forwards: id i's bytes (at 2n+2i) are at
        // or past where id i goes (4i), and read before it is written.
        let raw = ids.add(4 * n - w * n);
        let got = S::read(fp, raw, w * n);
        let byte = |i: usize| if i < got { i32::from(*raw.add(i)) } else { -1 };
        for i in 0..n {
            let id = if w == 4 { combine(4, |k| byte(4 * i + k)) } else { combine(2, |k| byte(2 * i + k)) & 0xFFFF };
            write(ids as usize + 4 * i, id);
        }
    }
}

/// Checks that every caller of the n-byte reader `f` ignores its result ([`read_keyed`] returns none): `known` holds
/// for the code after each call.
fn result_unused(core: &Image, f: usize, known: impl Fn(usize) -> Result<bool>) -> Result<()> {
    let calls = core.calls_to(f)?;
    ensure!(!calls.is_empty(), ".dmb byte reader: no callers");
    for &c in calls.iter() {
        ensure!(known(c + 5)?, ".dmb byte reader: caller at +{:#x} may use its result", c - core.base);
    }
    Ok(())
}

fn print_stats() {
    #[cfg(windows)]
    windows::print_stats();
    #[cfg(target_os = "linux")]
    linux::print_stats();
}
