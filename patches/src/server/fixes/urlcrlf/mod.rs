//! `urlcrlf` (forum report 2989601): `url_encode()` and `list2params()` encode a newline as `%0a`, not `%0d%0a`.
//!
//! BYOND's URL encoder special-cases `\n` and writes the six bytes `%0d%0a`. The fix removes that test, so `\n` takes
//! the generic path, which writes `%0a`.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static URLCRLF: Feature = Feature::fix(
    Hosts::SERVER,
    "urlcrlf",
    "url_encode newline as %0a",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);
