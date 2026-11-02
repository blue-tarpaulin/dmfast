//! `varlimit` (forum reports 2991568 and 2991894): with about 65,400 distinct variable names, one variable no longer
//! gets a false "variable defined but not used" warning, with its uses compiled as `.`.
//!
//! A var path is an array of 12-byte elements (id, flags, extra). The `$loc` path is one element with id `locIdx`
//! (0xFFD0) and the register flag set. Name ids count up from 0 and can reach `locIdx`, and the var reference codegen
//! tests only the id, so that variable compiles as `.`. The fix also tests the register flag, as BYOND's own fix does.
//! The id, flag, and flags offset are read from the code that builds the `$loc` path. Once BYOND ships its fix, the
//! old check is gone and this turns itself off.

use crate::base::code::{Cond, Stub};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;
use crate::base::{Result, bail};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static VARLIMIT: Feature = Feature::fix(
    Hosts::COMPILER,
    "varlimit",
    "a variable with id 0xFFD0 is no longer read as .",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// The `$loc` element: its id, the register flag, and the flags field's offset in an element.
struct Loc {
    id: u32,
    flag: u32,
    flags_at: u8,
}

/// Replace the check `cmp dword [eax],<locIdx> ; jne other` (`len` bytes at `cmp`, then `back`) with one that also
/// tests the register flag.
fn patch(core: &Image, cmp: usize, len: usize, other: usize, loc: Loc) -> Result<String> {
    let Loc { id, flag, flags_at } = loc;
    if !(0xFF00..0x10000).contains(&id) {
        bail!("the $loc check compares with {id:#x}, not a register id");
    }
    if flag.count_ones() != 1 || flag > 0x80 {
        bail!("$loc flag {flag:#x} is not a single low bit");
    }
    if flags_at > 0x7F {
        bail!("element flags at {flags_at:#x}");
    }
    let back = cmp + len;
    let stub = Stub::new()
        .bytes(&[&[0x81, 0x38][..], &id.to_le_bytes()].concat()) // cmp dword [eax],locIdx
        .jcc(Cond::NotEqual, other) // jne other
        .bytes(&[0xF6, 0x40, flags_at, flag as u8]) // test byte [eax+flags],flag
        .jcc(Cond::Equal, other) // je other
        .jmp(back) // jmp back
        .exec()?;
    core.untouched(&[(cmp, len)])?;
    core.jump(cmp, len, stub)?;
    Ok(format!("at +{:#x} (id {id:#x}, flag {flag:#x})", cmp - core.base))
}
