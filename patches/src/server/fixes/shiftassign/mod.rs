//! `shiftassign` (forum report 2992876): `A <<= B` and `A >>= B` on a datum with a shift overload (`operator<<`,
//! `operator<<=`, ...) store the overload's result in `A`, as `+=` and the other compound assignments do, instead of
//! keeping `A`.
//!
//! The shift-assign opcodes' overload case calls the overload and lets go of `A`'s old value, then moves on to the
//! next instruction without the write back to `A` that every other compound assignment ends with. The fix sends it on
//! to the end of `+=`'s overload case, which writes the result to the var, list item or other place the operator read
//! `A` from. Safe: at that point both cases hold the same thing in the same places (the result in the slot `+=`
//! writes, the opcode's position saved), so the write is the one `+=` makes with its overload's result.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static SHIFTASSIGN: Feature = Feature::fix(
    Hosts::SERVER,
    "shiftassign",
    "`<<=` and `>>=` store a shift overload's result",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
)
.fixed_in(516_1690, "https://www.byond.com/forum/post/2992876");
