//! `nulldiv` (forum report 2947411): `null / 0` and `null / null` are a "Division by zero" error, not 0.
//!
//! The `/` handler's null case now checks the right operand as the number case does: a number goes through its zero
//! check, null is a division by zero, and anything else takes stock's path.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static NULLDIV: Feature = Feature::fix(
    Hosts::SERVER,
    "nulldiv",
    "null / 0 is a division by zero",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

/// A number's and null's type tags.
const NUMBER: u8 = 0x2A;
const NULL: u8 = 0;
