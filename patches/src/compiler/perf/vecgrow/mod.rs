//! `vecgrow`: the compiler's dword-vector append grows the vector geometrically.
//!
//! The append (vector: `+0` data, `+4` capacity, `+8` size) reallocates to exactly `size + 1` when full, so each
//! append past the end reallocates. The grow step now sets the capacity to `2 * size + 4`. Capacity only decides when
//! to reallocate, so contents and size are unchanged. `deps` guards the rest of the function.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static VECGROW: Feature = Feature::perf(
    Hosts::COMPILER,
    "vecgrow",
    "vector growth",
    install!(windows: windows::install, linux: linux::install),
);
