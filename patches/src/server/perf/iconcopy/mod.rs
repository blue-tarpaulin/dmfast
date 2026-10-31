//! `iconcopy`: copying an icon no longer makes and frees a zeroed pixel buffer per frame.
//!
//! `DMGraphic::operator=` gives each copied frame a new pixel holder with a zeroed buffer of the world's icon size,
//! then drops it to share the source frame's holder. The frame now starts on the source's holder (count +1) with the
//! constructor's field values, so the assignment finds the holders equal and every frame ends up as stock leaves it.
//!
//! No dependency guard: the signatures pin the copy loop, the assignment, and the constructor code it replaces.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static ICONCOPY: Feature = Feature::perf(
    Hosts::SERVER,
    "iconcopy",
    "icon state copy",
    install!(windows: windows::install, linux: linux::install),
);

/// How far into `DMGraphic::operator=` the frame loop may start.
const SPAN: usize = 0x200;
