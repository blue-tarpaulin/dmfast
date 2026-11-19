//! `visleak`: an image added to and removed from `vis_contents` no longer leaks memory.
//!
//! Everything that can be in `vis_contents` keeps a list of the things whose `vis_contents` it is in. When that list
//! empties, BYOND frees it for atoms, but its function for that has no case for images, so each image put in a
//! `vis_contents` and taken out keeps its emptied list for good. The fix frees it for images the way BYOND does for
//! atoms.
//!
//! Safe: BYOND's own list release, on the image's vis record, which BYOND's other image code uses.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(windows)]
mod windows;

pub static VISLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "visleak",
    "an image's emptied vis_locs list is freed",
    Tier::Safe,
    install!(windows: windows::install),
);
