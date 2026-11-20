//! `outputleak` (forum report 2992664): `..()` in an `operator<<` or `operator>>` overload no longer keeps a
//! reference to its args, so they (a `/sound` sent with `usr << sound`, say) are freed.
//!
//! `..()` in an overload with no parent runs the operator's built-in code on the args it was given, which `..()` holds
//! references to. Every other way out of it releases them; through 516.1688 the ones for `<<` and for `>>` reading a
//! savefile or client return without doing so, and on every build `>>` from anything else ("bad input src") raises
//! its runtime error first, which skips the release. The fix releases them on each of those ways out: after the output
//! or input, and before the "bad input src" error, which only reads the source's type.
//!
//! Safe: each release comes after the args' last use (the error only looks at the type it was given), and the release
//! nulls the slots, so a later release of the same args, as BYOND 516.1689 does after input, finds nothing.

use crate::base::Result;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::server::values;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static OUTPUTLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "outputleak",
    "<< and >> overloads calling ..() release their args",
    Tier::Safe,
    install!(windows: install, linux: install),
)
.since(516_1665);

fn install(cx: &Ctx) -> Result<String> {
    let release = values::release(cx)?;
    #[cfg(windows)]
    let sites = windows::install(cx.core(), release)?;
    #[cfg(target_os = "linux")]
    let sites = linux::install(cx.core(), release)?;
    Ok(cx.core().report(&sites))
}
