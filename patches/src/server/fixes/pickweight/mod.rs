//! `pickweight` (forum report 2991804): `pick(1;"a", 0;"b")` no longer crashes the world.
//!
//! The compiler writes `pick()` as `PICK, n, (threshold, target) * n, final target`, leaving out options with a
//! constant 0 weight. When the last weight is 0, the final target slot holds the last kept option's threshold, with
//! its target after it, and the interpreter jumps to the threshold. The fix jumps to the word after it.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static PICKWEIGHT: Feature = Feature::fix(
    Hosts::SERVER,
    "pickweight",
    "pick() with a trailing 0 weight works",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// A final target at least this high is a threshold: the last threshold is at least 65535 - n, so this holds for up
/// to 2047 options, and a real target this high would need a proc over 63488 words.
const THRESHOLD: u16 = 0xF800;
