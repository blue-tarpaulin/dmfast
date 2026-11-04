//! `finalpath` (forum report 2992317): `final` before a proc or var in a type's path (`/foo/final/bar()`,
//! `/foo/final/proc/bar()`, `/foo/final/var/x`, or a `final` block holding them) is the `final` modifier, not a type
//! `/foo/final`.
//!
//! The compiler reads modifiers only after `proc`, `verb`, or `var`; before them every path element is a type, so
//! `final` there defined a type `/foo/final` holding the procs and vars, none of them final. Before the definition
//! passes (at the parent-type pass), each `final` node below a type whose children are all proc overrides and
//! `proc`, `verb`, or `var` blocks is taken out of the tree: a block becomes a `final` node in the type's own block of
//! that kind (`/foo/final/proc/bar()` is then `/foo/proc/final/bar()`), and an override becomes the type's, kept in a
//! list of final overrides. What replaces the `final` node takes its place among the type's children, so the others
//! keep their positions whenever the child count is unchanged. The override check's lookup of the overridden proc also
//! looks for the nearest final override (on a parent type, or earlier on the same type) and returns it as a final
//! definition, so overriding it gives BYOND's "override not allowed" and "final definition" errors. A `final` at the
//! top level, one without children, or one holding anything else (var overrides, subtypes) is still a type.
//!
//! Projects without `final` before a proc or var compile to the same `.dmb`: the tree is unchanged and the list empty.

use std::ops::RangeInclusive;
use std::sync::Mutex;

use crate::base::Result;
use crate::base::code::{Foreign, read, write};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::{Installed, lock};
use crate::compiler::names::Names;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static FINALPATH: Feature = Feature::fix(
    Hosts::COMPILER,
    "finalpath",
    "final before a proc or var in a path is a modifier, not a type",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

/// The name ids the fix tells nodes apart by, looked up in the name table at install.
struct Ids {
    /// `..`
    parent_link: u32,
    /// `.child_type`
    child_type: u32,
    /// `+` through `.index`.
    operators: RangeInclusive<u32>,
    var: u32,
    verb: u32,
    proc: u32,
    /// `.arg`
    arg: u32,
    /// `final`
    fin: u32,
}

impl Ids {
    fn new(names: &Names) -> Result<Self> {
        Ok(Self {
            parent_link: names.id("..")?,
            child_type: names.id(".child_type")?,
            operators: names.id("+")?..=names.id(".index")?,
            var: names.id("var")?,
            verb: names.id("verb")?,
            proc: names.id("proc")?,
            arg: names.id(".arg")?,
            fin: names.id("final")?,
        })
    }
}

/// A definition's flag for `final`.
const FINAL_FLAG: u32 = 0x20_0000;
/// A node's value kind byte (0: none).
const VALUE: usize = 4;

#[cfg(windows)]
type Ctor = unsafe extern "thiscall" fn(usize);
#[cfg(windows)]
type AddChild = unsafe extern "thiscall" fn(usize, usize);
#[cfg(windows)]
type EffParent = unsafe extern "fastcall" fn(usize) -> usize;
#[cfg(target_os = "linux")]
type Ctor = unsafe extern "C" fn(usize);
#[cfg(target_os = "linux")]
type AddChild = unsafe extern "C" fn(usize, usize);
#[cfg(target_os = "linux")]
type EffParent = unsafe extern "C" fn(usize) -> usize;
type Alloc = unsafe extern "C" fn(usize) -> usize;
/// The overridden definition's lookup: (node in, definition out; its flags; whether it's built in) -> found.
type Lookup = unsafe extern "C" fn(*mut usize, *mut u32, *mut u8) -> u8;

/// Where a tree node keeps its parent, child array, child count (u16), and flags byte (bit 0: defined), and the name
/// ids of the nodes told apart.
struct Layout {
    parent: usize,
    children: usize,
    count: usize,
    flags: usize,
    ids: Ids,
}

/// What install found: the node layout and size, the compiler's node allocator, constructor, child append, and
/// parent type (its `parent_type` or its parent node), and the stock lookup.
struct Found {
    layout: Layout,
    size: usize,
    alloc: Foreign<Alloc>,
    ctor: Foreign<Ctor>,
    add_child: Foreign<AddChild>,
    parent_type: Foreign<EffParent>,
    lookup: Foreign<Lookup>,
}

static FOUND: Installed<Found> = Installed::new();
/// The current compile's final overrides.
static FINALS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Node access. All take live nodes of the compiler's tree.
impl Layout {
    unsafe fn id(&self, n: usize) -> u32 {
        unsafe { read(n) }
    }

    unsafe fn parent(&self, n: usize) -> usize {
        unsafe { read::<u32>(n + self.parent) as usize }
    }

    unsafe fn count(&self, n: usize) -> usize {
        unsafe { usize::from(read::<u16>(n + self.count)) }
    }

    unsafe fn child(&self, n: usize, i: usize) -> usize {
        unsafe { read::<u32>(read::<u32>(n + self.children) as usize + 4 * i) as usize }
    }

    unsafe fn index(&self, n: usize, c: usize) -> Option<usize> {
        unsafe { (0..self.count(n)).find(|&i| self.child(n, i) == c) }
    }

    /// A proc definition: its first child is `var` with `.arg` first (its argument list).
    unsafe fn is_proc(&self, n: usize) -> bool {
        unsafe {
            self.count(n) > 0 && {
                let v = self.child(n, 0);
                self.id(v) == self.ids.var && self.count(v) > 0 && self.id(self.child(v, 0)) == self.ids.arg
            }
        }
    }

    /// A node that may be a type: a plain name without a value, not a proc, a block keyword, or a link.
    unsafe fn is_type(&self, n: usize) -> bool {
        unsafe {
            let (id, i) = (self.id(n), &self.ids);
            id != 0
                && ![i.var, i.verb, i.proc, i.parent_link, i.child_type].contains(&id)
                && !i.operators.contains(&id)
                && read::<u8>(n + VALUE) == 0
                && !self.is_proc(n)
        }
    }

    /// Removes child `i` of `n`.
    unsafe fn remove(&self, n: usize, i: usize) {
        unsafe {
            let a = read::<u32>(n + self.children) as usize as *mut u32;
            let count = self.count(n);
            std::ptr::copy(a.add(i + 1), a.add(i), count - i - 1);
            write(n + self.count, (count - 1) as u16);
        }
    }
}

impl Found {
    /// Adds `c` to `n`'s children at `i` (the compiler's append, then moved into place).
    unsafe fn insert(&self, n: usize, i: usize, c: usize) {
        unsafe {
            self.add_child.get()(n, c);
            let count = self.layout.count(n);
            let a = std::slice::from_raw_parts_mut(read::<u32>(n + self.layout.children) as usize as *mut u32, count);
            a[i..].rotate_right(1);
        }
    }

    /// A new node named `id`.
    unsafe fn node(&self, id: u32) -> usize {
        unsafe {
            let n = self.alloc.get()(self.size);
            self.ctor.get()(n);
            write(n, id);
            n
        }
    }

    /// `n`'s child named `id` that is a block (not a proc or valued), if any.
    unsafe fn block(&self, n: usize, id: u32) -> Option<usize> {
        let l = &self.layout;
        unsafe {
            (0..l.count(n))
                .map(|i| l.child(n, i))
                .find(|&c| l.id(c) == id && read::<u8>(c + VALUE) == 0 && l.count(c) > 0 && !l.is_proc(c))
        }
    }

    /// Takes the `final` node at child `i` of type `t` out of the tree (see the module docs). Returns how many nodes
    /// now take its place.
    unsafe fn unwrap(&self, t: usize, i: usize, fin: usize, finals: &mut Vec<usize>) -> usize {
        let l = &self.layout;
        unsafe {
            let moved: Vec<usize> = (0..l.count(fin)).map(|j| l.child(fin, j)).collect();
            l.remove(t, i);
            let mut at = i;
            for c in moved {
                if l.is_proc(c) {
                    self.insert(t, at, c);
                    at += 1;
                    finals.push(c);
                    continue;
                }
                let kind = l.id(c);
                let block = match self.block(t, kind) {
                    Some(b) => b,
                    None => {
                        let b = self.node(kind);
                        self.insert(t, at, b);
                        at += 1;
                        b
                    }
                };
                write(c, l.ids.fin);
                self.add_child.get()(block, c);
            }
            at - i
        }
    }

    /// Unwraps the `final` nodes below type `t` and its subtypes.
    unsafe fn walk(&self, t: usize, top: bool, finals: &mut Vec<usize>) {
        let l = &self.layout;
        unsafe {
            let mut i = 0;
            while i < l.count(t) {
                let c = l.child(t, i);
                if !l.is_type(c) {
                    i += 1;
                } else if l.id(c) == l.ids.fin && !top && self.movable(c) {
                    i += self.unwrap(t, i, c, finals);
                } else {
                    self.walk(c, false, finals);
                    i += 1;
                }
            }
        }
    }

    /// A `final` node with children, all of them overrides or `proc`, `verb`, and `var` blocks.
    unsafe fn movable(&self, fin: usize) -> bool {
        let l = &self.layout;
        let blocks = [l.ids.var, l.ids.verb, l.ids.proc];
        unsafe {
            l.count(fin) > 0
                && (0..l.count(fin))
                    .map(|j| l.child(fin, j))
                    .all(|c| l.is_proc(c) || (blocks.contains(&l.id(c)) && read::<u8>(c + VALUE) == 0))
        }
    }

    /// Whether proc definition `x` is an override (no `proc` or `verb` above it).
    unsafe fn is_override(&self, x: usize) -> bool {
        let l = &self.layout;
        unsafe {
            let mut p = l.parent(x);
            while p != 0 {
                if [l.ids.proc, l.ids.verb].contains(&l.id(p)) {
                    return false;
                }
                p = l.parent(p);
            }
            true
        }
    }

    /// The final override that override `x` overrides: on `x`'s type before it, or nearest on a parent type.
    unsafe fn final_above(&self, x: usize, finals: &[usize]) -> Option<usize> {
        let l = &self.layout;
        unsafe {
            let (id, t) = (l.id(x), l.parent(x));
            let named = || finals.iter().copied().filter(move |&g| g != x && l.id(g) == id);
            if let Some(xi) = l.index(t, x)
                && let Some(g) = named().find(|&g| l.parent(g) == t && l.index(t, g).is_some_and(|gi| gi < xi))
            {
                return Some(g);
            }
            // A parent_type loop is an error the compiler reports; stop on one.
            let mut p = self.parent_type.get()(t);
            for _ in 0..10_000 {
                if p == 0 {
                    break;
                }
                if let Some(g) = named().find(|&g| l.parent(g) == p) {
                    return Some(g);
                }
                p = self.parent_type.get()(p);
            }
            None
        }
    }
}

/// Unwraps the `final` nodes in the tree under `root` and starts a new list of final overrides. Called before the
/// stock parent-type pass.
///
/// # Safety
/// `root` is the compiler's tree root, before the definition passes.
unsafe fn unwrap_finals(root: usize) {
    let f = FOUND.get();
    let mut finals = lock(&FINALS);
    finals.clear();
    // SAFETY: the tree under `root` (caller).
    unsafe { f.walk(root, true, &mut finals) };
}

/// The stock lookup of the definition `*node` overrides, then, for an override, the final override it overrides
/// instead, if any, with the final flag. A final override that isn't defined yet is returned without it: stock then
/// tries again in the next pass, when it is.
///
/// # Safety
/// As the stock lookup.
unsafe extern "C" fn lookup(node: *mut usize, flags: *mut u32, builtin: *mut u8) -> u8 {
    let f = FOUND.get();
    // SAFETY: the stock lookup's arguments (caller), and live nodes.
    unsafe {
        let x = *node;
        let found = f.lookup.get()(node, flags, builtin);
        if found == 0 || *builtin != 0 {
            return found;
        }
        let finals = lock(&FINALS);
        if finals.is_empty() || !f.is_override(x) {
            return found;
        }
        if let Some(g) = f.final_above(x, &finals) {
            *node = g;
            if read::<u8>(g + f.layout.flags) & 1 != 0 {
                *flags |= FINAL_FLAG;
            } else {
                *flags &= !FINAL_FLAG;
            }
        }
        found
    }
}
