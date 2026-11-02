//! `objworld` (forum report 2960599): a type named `world` below another type (`pm/world`) keeps its var overrides
//! and procs.
//!
//! The definition pass treats any node with name id 0x13 as the built-in world (`cmp dword [node],13h`, twice) and
//! drops its unknown var entries. The fix also requires the node's parent to be the root (the only parentless node).
//! The parent field's offset is read from the type path builder.

use crate::base::Result;
use crate::base::code::{branch, exec_code, jcc};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static OBJWORLD: Feature = Feature::fix(
    Hosts::COMPILER,
    "objworld",
    "a type named world below another type is ordinary",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// Register numbers for the node.
const EAX: u8 = 0;
#[cfg(target_os = "linux")]
const EDX: u8 = 2;
#[cfg(target_os = "linux")]
const EDI: u8 = 7;

/// A `cmp dword [node],13h` at `at`, replaced up to `back`: `copy` bytes after it (they don't touch the flags) go
/// into the stub as they are, then the replaced conditional jump, if any (`0F op` and its target).
struct Site {
    at: usize,
    node: u8,
    copy: usize,
    jcc: Option<(u8, usize)>,
    back: usize,
}

/// Point `site` at a stub: `cmp dword [node],13h`, then "and the node's parent is the root" (ZF set for a world node,
/// or a node with no parent, as stock), the copied bytes, the conditional jump, and a jump back. ecx is kept.
fn redirect(core: &Image, site: Site, parent: u8) -> Result<()> {
    let Site { at, node, copy, jcc: cond, back } = site;
    let copied = core.bytes(at + 3, copy)?.to_vec();
    let len = 18 + copy + if cond.is_some() { 6 } else { 0 } + 5;
    let stub = exec_code(len, |s| {
        let mut v = vec![0x83, 0x38 | node, 0x13]; // cmp dword [node],13h
        v.extend([0x75, 0x0D]); // jne done
        v.push(0x51); // push ecx
        v.extend([0x8B, 0x48 | node, parent]); // mov ecx,[node+parent]
        v.extend([0x85, 0xC9]); // test ecx,ecx
        v.extend([0x74, 0x04]); // je pop
        v.extend([0x83, 0x79, parent, 0x00]); // cmp dword [ecx+parent],0
        v.push(0x59); // pop: pop ecx
        v.extend(&copied); // done:
        if let Some((op, target)) = cond {
            v.extend(jcc(op, s + v.len(), target));
        }
        v.extend(branch(0xE9, s + v.len(), back, 5));
        v
    })?;
    core.untouched(&[(at, back - at)])?;
    core.jump(at, back - at, stub)
}
