//! `ctype`: `isalpha`, `isdigit`, and the other character classes answer 0..127 from tables instead of the CRT's
//! per-call locale lookup (Windows).
//!
//! Only call sites that test the result for zero are redirected, so returning 1 instead of the CRT's mask changes
//! nothing, and classification below 0x80 is the same in every locale. EOF and bytes above 127 still go to the CRT.
//!
//! No dependency guard: it only redirects call sites its pattern matches whole.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod windows;

pub static CTYPE: Feature =
    Feature::perf(Hosts::COMPILER, "ctype", "char classes", install!(windows: windows::install));
