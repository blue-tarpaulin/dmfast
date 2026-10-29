//! `turfvar`: new turf var records are linked at the tail of their hash chain, not the head.
//!
//! Records are bucketed by the low 16 bits of the turf's hash, so a chain holds one x, y on every z. Stock links new
//! records at the head, and each `world.maxz++` rebuild reverses the chains, leaving the first levels deepest. The
//! search loop is rewritten in place to remember the last link, so a miss inserts at the tail.
//!
//! Only this function links records; everything else looks them up by key or walks them all, so lookups match
//! stock. `deps` guards the function, its globals, and the record walker.

use crate::base::feature::{Feature, Hosts, install};

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
