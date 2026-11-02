//! `objworld` (forum report 2960599): a type named `world` below another type (`pm/world`) keeps its var overrides
//! and procs.
//!
//! The definition pass treats any node with name id 0x13 as the built-in world (`cmp dword [node],13h`, twice) and
//! drops its unknown var entries. The fix also requires the node's parent to be the root (the only parentless node).
//! The parent field's offset is read from the type path builder.

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::{Branch, Image};

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
/// into the stub as they are, then the replaced conditional jump, if any ([`Image::jcc_at`]).
struct Site {
    at: usize,
    node: u8,
    copy: usize,
    jcc: Option<Branch>,
    back: usize,
}

/// The node's parent field offset, from the type path builder: the one match of `builder` whose call at `recurse`
/// calls itself, with the offset at `parent`.
fn parent_offset(core: &Image, builder: &str, parent: usize, recurse: usize) -> Result<u8> {
    let f = core.probe_unique(builder, "type path builder", |f| core.rel32(f + recurse).is_ok_and(|t| t == f))?;
    core.u8_at(f + parent)
}

/// Point `site` at a stub: `cmp dword [node],13h`, then "and the node's parent is the root" (ZF set for a world node,
/// or a node with no parent, as stock), the copied bytes, the conditional jump, and a jump back. ecx is kept.
fn redirect(core: &Image, site: Site, parent: u8) -> Result<()> {
    let Site { at, node, copy, jcc, back } = site;
    let copied = core.bytes(at + 3, copy)?.to_vec();
    let mut s = Stub::new();
    let (done, pop) = (s.label(), s.label());
    s.bytes(&[0x83, 0x38 | node, 0x13]) // cmp dword [node],13h
        .jcc8(Cond::NotEqual, done) // jne done
        .bytes(&[0x51]) // push ecx
        .bytes(&[0x8B, 0x48 | node, parent]) // mov ecx,[node+parent]
        .bytes(&[0x85, 0xC9]) // test ecx,ecx
        .jcc8(Cond::Equal, pop) // je pop
        .bytes(&[0x83, 0x79, parent, 0x00]) // cmp dword [ecx+parent],0
        .bind(pop)
        .bytes(&[0x59]) // pop: pop ecx
        .bind(done)
        .bytes(&copied); // done:
    if let Some(Branch { cond: Some(cc), target, .. }) = jcc {
        s.jcc(cc, target);
    }
    let stub = s.jmp(back).exec()?;
    core.untouched(&[(at, back - at)])?;
    core.jump(at, back - at, stub)
}
