//! `banner`: the compiler's version banner gets a dmfast line with the applied patch and fix counts.
//!
//! Dependency guard: none, it only changes operands its signatures match.

use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use crate::base::code::hx;
use crate::base::driver::Counts;
use crate::base::feature::{Ctx, Feature, Hosts, install};
use crate::base::{Result, bail};

pub static BANNER: Feature =
    Feature::perf(Hosts::COMPILER, "banner", "version banner", install!(windows: install, linux: install))
        .offered_if(super::found);

/// Version banners as in the binary
const BANNERS: [&[u8]; 2] = [b"DM compiler version %ld.%ld\n\n\0", b"DM compiler version %ld.%ld\n\0"];

/// Replacement banners
static BANNER_BUFS: [[AtomicU8; 128]; 2] = [const { [const { AtomicU8::new(0) }; 128] }; 2];

/// Writes the replacement banner: the compiler's line, then
/// `  dmfast <version> (<counts>) - Greytide Worldwide 2026`
fn write_banners(counts: &str) {
    let first: &[u8] = b"DM compiler version %ld.%ld\n";
    let line = format!("  dmfast {} ({counts}) - Greytide Worldwide 2026\n", env!("CARGO_PKG_VERSION"));
    for (orig, buf) in BANNERS.into_iter().zip(&BANNER_BUFS) {
        let text = [first, line.as_bytes(), &orig[first.len()..]].concat();
        // ~90 bytes; the original's NUL stays at the end.
        if text.len() <= buf.len() {
            for (cell, &b) in buf.iter().zip(&text) {
                cell.store(b, Relaxed);
            }
        }
    }
}

/// Point the `push <format>` of each version banner at our copy; fails only if none is found.
fn install(cx: &Ctx) -> Result<String> {
    let exe = cx.exe()?;
    write_banners("?");
    let mut parts = cx.parts();
    for (orig, buf) in BANNERS.into_iter().zip(&BANNER_BUFS) {
        parts.part(|| {
            let push = match exe.find_bytes(orig)?.as_slice()[..] {
                [s] => exe.find_unique(&format!("68 {}", hx(s)), "banner push")?,
                _ => bail!("banner {:?} not found once", String::from_utf8_lossy(orig)),
            };
            exe.write_code(push + 1, &(buf.as_ptr() as u32).to_le_bytes())?;
            Ok(format!("{:?}", String::from_utf8_lossy(orig).trim_end_matches(['\n', '\0'])))
        });
    }
    parts.finish()
}

/// `after_install` of [`crate::base::driver::Config`]
pub fn finish_banners(n: Counts) {
    if !BANNER.applied() {
        return;
    }
    let (applied, offered) = (n.applied - 1, n.offered - 1);
    write_banners(&match n.fixes {
        0 => format!("{applied}/{offered}"),
        1 => format!("{applied}/{offered}, 1 fix"),
        fixes => format!("{applied}/{offered}, {fixes} fixes"),
    });
}
