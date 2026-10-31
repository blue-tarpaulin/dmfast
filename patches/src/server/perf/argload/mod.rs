//! `argload`: the hot `Value` IncRef, DecRef, and assign functions read their 8-byte stack argument with two dword
//! loads instead of one qword load. Callers store it as two dwords, which a qword load can't be forwarded from, so
//! every call stalled.
//!
//! Linux only: MSVC's Windows build already uses dword loads.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;

pub static ARGLOAD: Feature =
    Feature::perf(Hosts::SERVER, "argload", "Value argument loads", install!(linux: linux::install));
