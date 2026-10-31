//! `cmperr` (forum reports 2989336 and 2983414): `<`, `>`, `<=`, and `>=` with anything but null, text, or a number
//! on the right are a "type mismatch: cannot compare" runtime error, instead of silently returning the left operand
//! (`35 < list()` was 35).
//!
//! Each comparison handler sends other right-hand types to a shared "skip this operation" tail. The fix sends that
//! jump to a stub that reloads the left operand, as the handler's own error block expects, and jumps to the block.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static CMPERR: Feature = Feature::fix(
    Hosts::SERVER,
    "cmperr",
    "< > <= >= error unless the right side is null, text or a number",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

/// The fewest comparison handlers expected: `<`, `>`, `<=`, and `>=`.
const HANDLERS: usize = 4;
