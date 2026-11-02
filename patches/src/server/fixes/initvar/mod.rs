//! `initvar` (forum report 2924945): `initial(x.var)` on a text or number value is an error, as `x.var` is, not a
//! silent null.
//!
//! `initial()` sends such values to the ordinary var read, which raises the error and returns null. Other values take
//! stock's path. The text and number tags come from `minmax`'s checked reading of `max` and `min`.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static INITVAR: Feature = Feature::fix(
    Hosts::SERVER,
    "initvar",
    "initial() of a text/number var errors",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);
