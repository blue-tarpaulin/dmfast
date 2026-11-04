//! `crc`: BYOND's CRC-32 eight bytes per step instead of one.
//!
//! BYOND's `crc(seed, data, len)` is an MSB-first CRC that takes one byte per step through a 256-entry table.
//! [`checksum`] takes eight per step (slicing-by-8) with tables derived from BYOND's own, so it returns the same CRC
//! for every input. `DMFAST_VERIFY=1` also runs the original, answers with it, and counts disagreements.

use std::sync::LazyLock;

use crate::base::code::{Foreign, read};
use crate::base::driver::verify;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static CRC: Feature =
    Feature::perf(Hosts::SERVER, "crc", "CRC-32", install!(windows: windows::install, linux: linux::install))
        .stats(verify::print_stats);

/// A CRC entry point (cdecl: seed, data, length -> CRC).
type CrcFn = unsafe extern "C" fn(u32, *const u8, u32) -> u32;

/// What install found.
struct Found {
    /// BYOND's table (256 dwords), and the flag byte set once it is filled.
    table: usize,
    ready: usize,
    /// Fills the table.
    init: Foreign<unsafe extern "C" fn()>,
    /// The original CRC.
    original: Foreign<CrcFn>,
}

static FOUND: Installed<Found> = Installed::new();

/// BYOND's CRC function, hooked: `strtable` checks that its calls go there.
#[cfg(windows)]
pub(crate) static HOOKED: Installed<usize> = Installed::new();

/// The eight slicing tables, derived from BYOND's on first use.
static SLICES: LazyLock<[[u32; 256]; 8]> = LazyLock::new(|| {
    let f = FOUND.get();
    let mut t = [[0u32; 256]; 8];
    // SAFETY: install found BYOND's flag byte, its table, and the function filling it.
    t[0] = unsafe {
        // As the original: fill BYOND's table first if it isn't yet.
        if read::<u8>(f.ready) == 0 {
            f.init.get()();
        }
        read(f.table)
    };
    for k in 1..8 {
        for i in 0..256 {
            let prev = t[k - 1][i];
            t[k][i] = (prev << 8) ^ t[0][(prev >> 24) as usize];
        }
    }
    t
});

/// BYOND's CRC of `bytes` from `seed`.
fn checksum(seed: u32, bytes: &[u8]) -> u32 {
    let t = &*SLICES;
    let mut c = seed;
    let (chunks, rest) = bytes.as_chunks::<8>();
    for b in chunks {
        let hi = c ^ u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
        c = t[7][(hi >> 24) as usize]
            ^ t[6][(hi >> 16 & 0xFF) as usize]
            ^ t[5][(hi >> 8 & 0xFF) as usize]
            ^ t[4][(hi & 0xFF) as usize]
            ^ t[3][(lo >> 24) as usize]
            ^ t[2][(lo >> 16 & 0xFF) as usize]
            ^ t[1][(lo >> 8 & 0xFF) as usize]
            ^ t[0][(lo & 0xFF) as usize];
    }
    for &b in rest {
        c = (c << 8) ^ t[0][((c >> 24) ^ b as u32) as usize];
    }
    c
}

/// [`checksum`] as BYOND's CRC (cdecl: seed, data, length -> CRC).
///
/// # Safety
/// `data..data+len` is readable.
pub(crate) unsafe extern "C" fn crc(seed: u32, data: *const u8, len: u32) -> u32 {
    // SAFETY: readable (caller).
    checksum(seed, if len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(data, len as usize) } })
}

/// Replaces BYOND's CRC: [`crc`], checked against the original under `DMFAST_VERIFY=1`.
///
/// # Safety
/// As [`crc`].
unsafe extern "C" fn entry(seed: u32, data: *const u8, len: u32) -> u32 {
    // SAFETY: readable (caller).
    let ours = unsafe { crc(seed, data, len) };
    if !verify() {
        return ours;
    }
    // SAFETY: as above.
    unsafe { verify::original(ours, seed, data, len) }
}
