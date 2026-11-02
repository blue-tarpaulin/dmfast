//! `minmax` (forum report 2816779): `max()` and `min()` treat null as 0 against a number, so `max(-1, null)` is 0,
//! not null, and as no value against text, so the text wins whatever the argument order. Null against null stays
//! null.
//!
//! Hooks on `max(a, b)` and `min(a, b)` handle those cases and pass everything else to the original. The number and
//! text tags come from the functions' own type tests, which must match on both, so a build that changed them doesn't
//! install.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static MINMAX: Feature = Feature::fix(
    Hosts::SERVER,
    "minmax",
    "min/max treat null as 0",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

/// Null's type tag.
const NULL: u8 = 0;

/// The type tags of numbers and text, which `initvar` also needs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tags {
    pub number: u8,
    pub text: u8,
}

// `value_tags(core)`: reads [`Tags`] from `max` and `min`, also after this fix hooked them.
#[cfg(target_os = "linux")]
pub(crate) use linux::value_tags;
#[cfg(windows)]
pub(crate) use windows::value_tags;
