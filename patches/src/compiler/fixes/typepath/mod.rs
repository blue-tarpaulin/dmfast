//! `typepath` (forum report 2990024): operator names are no longer accepted as type-path elements, so `/obj/!`,
//! `/obj/~`, and `/obj/?` don't define types that can be instantiated and called.
//!
//! The node-define routine rejects names with ids `+` through `goto`, but the operators after them (`- ? ~ ! ++ --
//! .post++ .post-- .reference .dereference .index`) pass. The fix raises the bound to the id of `.index`, read from
//! BYOND's name table. `/obj/:` and `/obj/;` are not reached through an id and are unchanged.

use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;
use crate::base::{Result, bail};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static TYPEPATH: Feature = Feature::fix(
    Hosts::COMPILER,
    "typepath",
    "operator names are not type-path elements",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// The operators after `goto` in BYOND's name table that the node-define routine lets through.
const OPERATORS: [&[u8]; 11] =
    [b"-", b"?", b"~", b"!", b"++", b"--", b".post++", b".post--", b".reference", b".dereference", b".index"];

/// The id of the last operator name (`.index`) in BYOND's name table at `names`; `plus` and `goto` are the check's
/// ids.
fn last_operator(core: &Image, names: usize, plus: usize, goto: usize) -> Result<usize> {
    let name = |id: usize| -> Result<Option<&[u8]>> {
        let p = core.ptr_at(names + id * 4)?;
        if p == 0 { Ok(None) } else { core.c_str(p).map(Some) }
    };
    if name(plus)? != Some(&b"+"[..]) || name(goto)? != Some(&b"goto"[..]) {
        bail!("name table: + is not id {plus:#x} or goto is not id {goto:#x}");
    }
    for (i, op) in OPERATORS.iter().enumerate() {
        if name(goto + 1 + i)? != Some(*op) {
            bail!("name table id {:#x} is not {}", goto + 1 + i, String::from_utf8_lossy(op));
        }
    }
    Ok(goto + OPERATORS.len())
}
