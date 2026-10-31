//! `getvar`: `argload`'s fix for two more functions, GetVariable (a value's variable by name) and the user var read
//! it calls: their 8-byte `Value` argument is read with two dword loads.
//!
//! Linux only, as `argload`. Every register, flag, and stack byte ends as stock leaves them; `deps` guards the rest of
//! both functions.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;

pub static GETVAR: Feature =
    Feature::perf(Hosts::SERVER, "getvar", "GetVariable argument loads", install!(linux: linux::install));
