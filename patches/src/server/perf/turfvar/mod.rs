//! `turfvar`: new turf var records are linked at the tail of their hash chain, not the head.
//!
//! Records are bucketed by the low 16 bits of the turf's hash, so a chain holds one x, y on every z. Stock links new
//! records at the head, and each `world.maxz++` rebuild reverses the chains, leaving the first levels deepest. The
//! search loop is rewritten in place to remember the last link, so a miss inserts at the tail.
//!
//! Only this function links records; everything else looks them up by key or walks them all, so lookups match
//! stock. `deps` guards the function, its globals, and the record walker.

use crate::base::feature::{Feature, Hosts, install};
use crate::base::image::Image;
use crate::base::{Result, bail};

/// Fails if a relocation or a direct branch (`E8`, `E9`, or `0F 8x`) from anywhere in `core` lands inside
/// `at..at+len`. A branch to `at` itself is fine.
fn untouched(core: &Image, at: usize, len: usize) -> Result<()> {
    if let Some(r) = core.reloc_sites().iter().find(|&&r| r + 4 > at && r < at + len) {
        bail!("turf var search: relocation at {r:#x} inside the rewritten range");
    }
    for (start, n) in core.code_ranges() {
        let bytes = core.haystack(start, n)?;
        for i in 0..n.saturating_sub(6) {
            let op = match bytes[i] {
                0xE8 | 0xE9 => 1,
                0x0F if bytes[i + 1] & 0xF0 == 0x80 => 2,
                _ => continue,
            };
            let rel = i32::from_le_bytes([bytes[i + op], bytes[i + op + 1], bytes[i + op + 2], bytes[i + op + 3]]);
            let t = (start + i + op + 4).wrapping_add(rel as isize as usize);
            if t > at && t < at + len {
                bail!("turf var search: branch at {:#x} into the rewritten range", start + i);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static TURFVAR: Feature = Feature::perf(
    Hosts::SERVER,
    "turfvar",
    "turf var chain order",
    install!(windows: windows::install, linux: linux::install),
);
