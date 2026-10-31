//! `dmbbuf`: the compiler writes the `.dmb` through a 1 MB stream buffer instead of the C library's 4 KB.
//!
//! The library's file opens go through a wrapper that gives a stream on a `.dmb` file the bigger buffer right after
//! opening, before any I/O. Buffering only changes when bytes reach the OS, so the file is unchanged.
//!
//! No dependency guard: it only wraps the open call.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::feature::{Feature, Hosts, install};
use crate::compiler::perf::fscache::FSCACHE;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static DMBBUF: Feature =
    Feature::perf(Hosts::COMPILER, "dmbbuf", "dmb buffer", install!(windows: windows::install, linux: linux::install))
        // Windows: wraps fscache's `_wfopen` hook.
        .after(&[&FSCACHE])
        .stats(print_stats);

const BUFFER: usize = 1 << 20;

/// Streams given the buffer.
static BUFFERED: AtomicU32 = AtomicU32::new(0);

/// Whether a file name (bytes or UTF-16 units) ends in `.dmb`, in any case.
fn is_dmb<T: Copy + Into<u32>>(name: &[T]) -> bool {
    let Some(ext) = name.last_chunk::<4>() else { return false };
    ext.iter().zip(b".dmb").all(|(&c, &e)| c.into() | 0x20 == u32::from(e))
}

fn print_stats() {
    eprintln!("dmfast: dmb buffer on {} streams", BUFFERED.load(Relaxed));
}
