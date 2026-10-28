//! `iconwrap` (forum report 2922816): an icon modified tens of thousands of times keeps rendering right.
//!
//! A runtime `/icon` keeps a list of the operations done to it, and the offset in that list it last rendered up to in
//! a 16-bit field. Past 64 KB the offset wraps, giving wrong pixels, "Unrecognized icon opcode", or a freeze. The
//! render function's store of the offset is replaced with [`store_offset`], which first flattens a list over 0xFFFF
//! bytes to BYOND's one-byte `?` entry ("start from the rendered copy"), as BYOND's own flatten does.

use crate::base::code::{Foreign, live_u32};
use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static ICONWRAP: Feature = Feature::fix(
    Hosts::SERVER,
    "iconwrap",
    "an icon's operation list over 64 KB is flattened before its offset wraps",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// BYOND's flatten (the list): its buffer becomes the `?` entry.
#[cfg(windows)]
type FlattenFn = extern "fastcall" fn(usize);
#[cfg(target_os = "linux")]
type FlattenFn = extern "C" fn(usize);

static FLATTEN: Foreign<FlattenFn> = Foreign::new();

/// Stores the icon record's rendered offset, flattening its operation list first if the offset can't hold its length.
/// Record: +8 the list (+0Ch its length), +14h the 16-bit offset.
extern "C" fn store_offset(record: usize) {
    // SAFETY: called by the render function with its icon record; FLATTEN was checked at install.
    unsafe {
        if live_u32(record + 0x0C) > 0xFFFF {
            FLATTEN.get()(record + 8);
        }
        ((record + 0x14) as *mut u16).write(live_u32(record + 0x0C) as u16);
    }
}
