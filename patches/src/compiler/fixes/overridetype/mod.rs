//! `overridetype` (forum report 2992612): a subtype named like a proc override on the same type (`/obj/a/open()` and
//! `/obj/a/open`) is a type of its own, whichever is defined first.
//!
//! An override is a node among its type's children, named after the proc like a subtype, and path resolution takes
//! the first child with the name. A subtype defined after the override was merged into the override's node: its var
//! overrides and procs became part of the proc's body, and `new /obj/a/open` made nothing. One defined before it was a
//! `duplicate definition`. Now each step of path resolution passes over an override: always where it creates the
//! nodes it doesn't find (definitions), and elsewhere when a sibling with the same name isn't a proc. Stock's own
//! retry then takes the next node with that name, or, in a definition, creates one. The definition pass's duplicate
//! check counts only the siblings that aren't procs.
//!
//! Projects without a subtype named like an override beside it compile to the same `.dmb`: no override is passed
//! over, and the duplicate check counts the same nodes.

use crate::base::code::{Foreign, read};
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::sync::Installed;
use crate::base::{Result, ensure};
use crate::compiler::layout::{CHILDREN, COUNT, PARENT};
use crate::compiler::names::Names;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static OVERRIDETYPE: Feature = Feature::fix(
    Hosts::COMPILER,
    "overridetype",
    "a subtype named like a proc override on its type is a type",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// Path resolution (cdecl): (node, path, index of the path's next element, flags) -> the node it resolves to, or 0.
type Resolve = unsafe extern "C" fn(usize, usize, u32, u32) -> usize;
/// The flag of a resolution that creates the nodes it doesn't find.
const CREATE: u32 = 8;

/// The name ids that make a proc definition, and stock's path resolution.
struct Found {
    var: u32,
    arg: u32,
    proc: u32,
    verb: u32,
    resolve: Foreign<Resolve>,
}

static FOUND: Installed<Found> = Installed::new();

/// Node access. All take live nodes of the compiler's tree.
impl Found {
    unsafe fn first_child(&self, n: usize) -> Option<usize> {
        unsafe { (read::<u16>(n + COUNT) > 0).then(|| read::<u32>(read::<u32>(n + CHILDREN) as usize) as usize) }
    }

    /// A proc definition: its first child is `var` with `.arg` first (its argument list). Not BYOND's test, which
    /// caches its answer on the node, and nodes are resolved before their argument list is added.
    unsafe fn is_proc(&self, n: usize) -> bool {
        unsafe {
            self.first_child(n).is_some_and(|v| {
                read::<u32>(v) == self.var && self.first_child(v).is_some_and(|a| read::<u32>(a) == self.arg)
            })
        }
    }

    /// A proc override: a proc definition whose parent is a type, not a `proc` or `verb` block.
    unsafe fn is_override(&self, n: usize) -> bool {
        unsafe {
            let p = read::<u32>(n + PARENT) as usize;
            p != 0 && ![self.proc, self.verb].contains(&read::<u32>(p)) && self.is_proc(n)
        }
    }

    /// How many of `n`'s siblings (`n` too) have its name and aren't procs.
    unsafe fn named_others(&self, n: usize) -> usize {
        unsafe {
            let p = read::<u32>(n + PARENT) as usize;
            if p == 0 {
                return 0;
            }
            let id = read::<u32>(n);
            let kids = std::slice::from_raw_parts(
                read::<u32>(p + CHILDREN) as usize as *const u32,
                usize::from(read::<u16>(p + COUNT)),
            );
            kids.iter().filter(|&&s| read::<u32>(s as usize) == id && !self.is_proc(s as usize)).count()
        }
    }
}

/// One step of path resolution, into `node`, a child its parent's step matched by name: stock's, but 0 (no match, so
/// stock tries the next child with the name, or creates one) for an override passed over.
///
/// # Safety
/// As stock's resolution.
unsafe extern "C" fn step(node: usize, path: usize, next: u32, flags: u32) -> usize {
    let f = FOUND.get();
    // SAFETY: a live node (caller).
    unsafe {
        if f.is_override(node) && (flags & CREATE != 0 || f.named_others(node) > 0) {
            return 0;
        }
        f.resolve.get()(node, path, next, flags)
    }
}

/// The definition pass's duplicate check: how many of `node`'s siblings (`node` too) have its name and aren't procs.
///
/// # Safety
/// Only called by the compiler, with a live node.
unsafe extern "C" fn duplicates(node: usize) -> u32 {
    // SAFETY: a live node (caller).
    unsafe { FOUND.get().named_others(node) as u32 }
}

/// Commits what install found and points the rel32 fields `step_call` (the resolution's call to itself for a matched
/// child) at [`step`] and `count_call` (the duplicate check's count) at [`duplicates`]. `layout` is the parent, child
/// array, and child count offsets the stock count reads.
fn commit(cx: &Ctx, layout: [u8; 3], resolve: usize, step_call: usize, count_call: usize) -> Result<()> {
    let core = cx.core();
    ensure!(layout.map(usize::from) == [PARENT, CHILDREN, COUNT], "duplicate count: another node layout");
    let names = Names::new(core)?;
    FOUND.commit(
        cx,
        Found {
            var: names.id("var")?,
            arg: names.id(".arg")?,
            proc: names.id("proc")?,
            verb: names.id("verb")?,
            resolve: Foreign::at(resolve),
        },
    );
    core.retarget(step_call, step as Resolve)?;
    core.retarget(count_call, duplicates as unsafe extern "C" fn(usize) -> u32)
}
