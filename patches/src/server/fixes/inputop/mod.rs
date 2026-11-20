//! `inputop` (forum report 2992664): an overloaded `>>` input statement (`A >> x` with `A.operator>>`) no longer
//! crashes DreamDaemon or corrupts its memory.
//!
//! The opcode passes the overload two args, null and the input, in an array on its frame. It nulls the first through
//! a frame slot that another opcode uses for a pointer, so it writes 8 zero bytes wherever that slot last pointed
//! (a crash on 516.1688, quiet heap damage on 516.1689) and the first arg is whatever was left in the array. The fix
//! loads the array's address instead, so the null lands in the first arg.
//!
//! Safe: the write goes where the opcode meant it to; nothing else reads the stale slot here.

use crate::base::Result;
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static INPUTOP: Feature = Feature::fix(
    Hosts::SERVER,
    "inputop",
    "overloaded >> input doesn't write through a stale pointer",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// Turns the route's `mov eax,[ebp+stale]` at `at` into `lea eax,[ebp+args]`, with the array's displacement read at
/// `disp`.
fn point_at(core: &Image, at: usize, disp: usize) -> Result<String> {
    let [a, b, c, d] = core.array::<4>(disp)?;
    core.write_code(at, &[0x8D, 0x85, a, b, c, d])?;
    Ok(core.report(&[at]))
}
