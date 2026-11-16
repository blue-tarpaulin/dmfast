//! `valret`: byondcore's small functions that build a `Value` to return (an id or null, a number, an id of a fixed
//! type) store its type byte into a stack slot and read the slot back as a dword into `eax`. A dword load right after
//! a byte store into it can't be forwarded from the store, so every call stalled until the store was written to the
//! cache.
//!
//! Each is rewritten in place to read the slot first, put the type into `al`, and then store the byte. `eax`, `ecx`,
//! `edx`, `xmm0`, and the stack end up exactly as before, including the slot's other three bytes (padding that
//! callers copy along).
//!
//! Windows only: libbyond returns a `Value` through a pointer its caller passes. No dependency guard: each
//! signature is the whole function.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod windows;

pub static VALRET: Feature =
    Feature::perf(Hosts::SERVER, "valret", "Value returns", install!(windows: windows::install));
