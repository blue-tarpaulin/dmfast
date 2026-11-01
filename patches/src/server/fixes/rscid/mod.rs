//! `rscid`: the 65,535th runtime icon a world creates no longer gets an id another icon already has.
//!
//! Runtime resources go in the dynamic set (dyn.rsc, kept across restarts) with id `base + index`. When that would
//! be 0xFFFF ("no resource"), BYOND moves the entry to the main set, whose lookup returns an id another resource
//! already has, so clients draw one icon as another.
//!
//! The fix wraps the dynamic set's insert where the get-or-add functions call it. When the next id would be 0xFFFF,
//! it first inserts BYOND's own empty sentinel key, which takes 0xFFFF, so the real key gets 0x10000 and the move is
//! never taken. Ids are 32 bits on the wire, and 0xFFFF is the only reserved one.

use crate::base::code::live_u32;
use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static RSCID: Feature = Feature::fix(
    Hosts::SERVER,
    "rscid",
    "two runtime icons never share a resource id",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// BYOND's resource key: the constructor fills it, then the 0xFFFF branch makes it the sentinel. 40h bytes hold
/// BYOND's own (at most 34h).
#[repr(C)]
struct Key {
    vtable: u32,
    _head: [u32; 7],
    length: i32,
    flags: u16,
    _tail: [u8; 0x1A],
}

impl Key {
    const EMPTY: Key = Key { vtable: 0, _head: [0; 7], length: 0, flags: 0, _tail: [0; 0x1A] };

    /// Make a constructed key the sentinel, as the 0xFFFF branch does: its vtable, length -1, and flags 0xFFFF.
    fn make_sentinel(&mut self, vtable: u32) {
        self.vtable = vtable;
        self.length = -1;
        self.flags = 0xFFFF;
    }
}

/// Whether the next insert into the dynamic set hands out id 0xFFFF: its id base (at +28h) plus its entry count (at
/// `count`; entries are never removed).
///
/// # Safety
/// `set` is the dynamic set and `count` its entry count's offset.
unsafe fn next_is_null(set: usize, count: usize) -> bool {
    // SAFETY: the set's fields (caller).
    unsafe { live_u32(set + 0x28).wrapping_add(live_u32(set + count)) == 0xFFFF }
}
