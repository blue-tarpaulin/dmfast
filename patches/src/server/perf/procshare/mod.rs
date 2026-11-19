//! `procshare`: each type's proc table holds only the names the type defines; a call by name walks up to its parents.
//!
//! When a .dmb loads, BYOND builds each type's table of the procs and verbs callable on it by name: a sorted array of
//! 12-byte entries, the type's own names with its parent's whole table merged in, so a table mostly repeats its
//! parent's (on tg ~460 MB of tables, of which the types' own names take ~2 MB). Here the builder's merge gets, for
//! each name the type defines, only the parent's answer for that name, so the table keeps the type's own names with
//! their whole override chains and `..()` links as BYOND's merge builds them. Calls by name on a type's table (the one
//! caller of the table getter) search the table and then its parents', and remember the answer, misses included.
//!
//! It's exact because a stock table is the type's own names as built here plus, for each inherited name, the entry of
//! the nearest parent defining it, unchanged (the merge copies it; it skips entries without procs, which answer
//! nothing either way). Tables are only written while a .dmb loads, which builds each once, parents first; from then
//! a built table and its parents' are fixed until the teardown, which drops the remembered answers. The answers are
//! the main thread's; other threads walk. `DMFAST_VERIFY=1` checks every remembered answer against a walk.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::procname::{self, LookupFn, NONE};
use crate::base::code::read;
use crate::base::driver::{Counter, verify};
use crate::base::feature::{Ctx, Feature, Hosts, install};
use crate::base::image::Image;
use crate::base::sync::{Installed, Owned};
use crate::base::{Result, ensure};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static PROCSHARE: Feature = Feature::perf(
    Hosts::SERVER,
    "procshare",
    "own proc tables",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// A type's proc table: `count` entries sorted by name; `built` once the builder ran for this load.
#[repr(C)]
struct Table {
    entries: *const Entry,
    count: u16,
    built: u8,
    _pad: u8,
}

/// A name callable on the type: its procs (a shared array of `count` proc ids, the type's override first) and flag.
#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    name: u32,
    values: u32,
    count: u16,
    flag: u8,
    _pad: u8,
}

impl Table {
    /// # Safety
    /// A live table.
    unsafe fn entries(&self) -> &[Entry] {
        if self.entries.is_null() {
            return &[];
        }
        // SAFETY: `count` entries at `entries` (caller).
        unsafe { std::slice::from_raw_parts(self.entries, self.count as usize) }
    }

    /// The entry for `name`.
    ///
    /// # Safety
    /// A live table.
    unsafe fn find(&self, name: u32) -> Option<&Entry> {
        // SAFETY: as the caller's.
        let e = unsafe { self.entries() };
        e.binary_search_by_key(&name, |e| e.name).ok().map(|i| &e[i])
    }
}

/// A type's (proto's) parent type id.
const PARENT: usize = 4;

/// The globals (addresses) of the types and their count, and of the proc tables (one per type); `proto` is a type's
/// size.
struct Globals {
    protos: usize,
    ntypes: usize,
    tables: usize,
    proto: usize,
}

static GLOBALS: Installed<Globals> = Installed::new();

const SLOT_BITS: u32 = 14;

/// A remembered answer: `name` in the table at `map`, during load number `load`, is `entry`.
#[derive(Clone, Copy, Default)]
struct Slot {
    map: usize,
    name: u32,
    load: u32,
    entry: Option<&'static Entry>,
}

struct Cache {
    slots: Box<[Slot]>,
    hits: u64,
    walks: u64,
}

static CACHE: Owned<Cache> = Owned::new();
/// Counts teardowns: answers remembered before one are stale.
static LOAD: AtomicU32 = AtomicU32::new(0);
static MERGES: Counter = Counter::new();

/// The id of the type whose table is at `map`, if it is a type's table.
///
/// # Safety
/// The globals are BYOND's.
unsafe fn type_of(map: usize) -> Option<usize> {
    let g = GLOBALS.get();
    // SAFETY: BYOND's globals (caller).
    let (tables, ntypes) = unsafe { (read::<usize>(g.tables), read::<usize>(g.ntypes)) };
    let off = map.wrapping_sub(tables);
    let ty = off / size_of::<Table>();
    (off % size_of::<Table>() == 0 && ty < ntypes).then_some(ty)
}

/// The entry for `name` in the table of type `ty` or, failing that, of the nearest parent with one.
///
/// # Safety
/// `ty`'s table and its parents' are built.
unsafe fn resolve(mut ty: usize, name: u32) -> Option<&'static Entry> {
    let g = GLOBALS.get();
    // SAFETY: BYOND's tables and types, indexed within their count (caller: built, so live until the teardown).
    unsafe {
        while ty < read::<usize>(g.ntypes) {
            let t = &*(read::<usize>(g.tables) as *const Table).add(ty);
            if let Some(e) = t.find(name) {
                return Some(e);
            }
            ty = read::<u32>(read::<usize>(g.protos) + ty * g.proto + PARENT) as usize;
        }
        None
    }
}

/// [`resolve`], remembered (on the cache's thread).
///
/// # Safety
/// As [`resolve`]; `map` is `ty`'s table.
unsafe fn resolve_cached(map: usize, ty: usize, name: u32) -> Option<&'static Entry> {
    // SAFETY: as the caller's.
    let Some(c) = CACHE.get() else { return unsafe { resolve(ty, name) } };
    let load = LOAD.load(Relaxed);
    let h = (map as u32 ^ name.wrapping_mul(0x9E37_79B9)).wrapping_mul(0x85EB_CA6B);
    let s = &mut c.slots[(h >> (32 - SLOT_BITS)) as usize];
    if (s.map, s.name, s.load) == (map, name, load) {
        c.hits += 1;
        if verify() {
            // SAFETY: as the caller's.
            verify::check(s.entry, unsafe { resolve(ty, name) });
        }
        return s.entry;
    }
    c.walks += 1;
    // SAFETY: as the caller's.
    let entry = unsafe { resolve(ty, name) };
    *s = Slot { map, name, load, entry };
    entry
}

/// Replaces BYOND's name lookup (map, name, depth, flag) where a type's table is looked up; the answer from the
/// entry is BYOND's own logic.
///
/// # Safety
/// `map` is a live table (BYOND passes it).
unsafe extern "C" fn lookup(map: usize, name: u32, depth: u32, flag: u32) -> u32 {
    // SAFETY: a live table (caller); a built one's parents are built.
    unsafe {
        let t = &*(map as *const Table);
        let e = match type_of(map) {
            Some(ty) if t.built != 0 => resolve_cached(map, ty, name),
            _ => t.find(name),
        };
        e.map_or(NONE, |e| procname::answer(std::ptr::from_ref(e) as usize, depth, flag))
    }
}

/// Replaces the builder's merge of the parent's table `src` into the type's `dst` (which holds the type's own names):
/// merges, with BYOND's `merge`, the parent's answers for those names instead.
///
/// # Safety
/// `dst` is the table being built, `src` its parent's table or not a type's (merged as is).
unsafe fn merge_parent(dst: *mut Table, src: *const Table, merge: impl FnOnce(*mut Table, *const Table)) {
    // SAFETY: as the caller's: the parent's table is built, and so are its parents'.
    unsafe {
        let Some(parent) = type_of(src as usize) else { return merge(dst, src) };
        MERGES.bump();
        let answers: Vec<Entry> = (*dst).entries().iter().filter_map(|e| resolve(parent, e.name)).copied().collect();
        let src = Table { entries: answers.as_ptr(), count: answers.len() as u16, built: 1, _pad: 0 };
        merge(dst, &src);
    }
}

/// Where one OS's code holds what [`find`] needs: each signature and the offsets of the globals in it.
struct Sigs {
    /// The builder, with the type count, types, and tables (and the size of a type).
    builder: &'static str,
    builder_globals: [usize; 3],
    proto: usize,
    /// The getter (type id -> table), with the type count and tables.
    getter: &'static str,
    getter_globals: [usize; 2],
    /// The teardown's loop over types, with the type count, types, and tables.
    teardown: &'static str,
    teardown_globals: [usize; 3],
}

/// What [`find`] found: the builder, the teardown's loop, the lookup call [`lookup`] replaces, and the globals.
struct Found {
    builder: usize,
    teardown: usize,
    site: usize,
    globals: Globals,
}

/// The builder, getter, and teardown, checked to use the same tables, and the getter's one caller's lookup call.
fn find(core: &Image, s: &Sigs) -> Result<Found> {
    let builder = core.find_unique(s.builder, "type proc table builder")?;
    let getter = core.find_unique(s.getter, "type proc table getter")?;
    let teardown = core.find_unique(s.teardown, "type proc table teardown")?;
    let [ntypes, protos, tables] = ptrs(core, builder, s.builder_globals)?;
    ensure!(
        ptrs(core, getter, s.getter_globals)? == [ntypes, tables]
            && ptrs(core, teardown, s.teardown_globals)? == [ntypes, protos, tables],
        "type proc table getter or teardown: not the builder's tables"
    );
    let lookup_fn = core.find_unique(procname::LOOKUP, "proc name lookup")?;
    let caller = core.calls_to(getter)?.unique("type proc table getter's caller")?;
    let site = core.calls_in(caller + 5, 0x40, lookup_fn)?.unique("lookup after the type proc table getter")?;
    Ok(Found { builder, teardown, site, globals: Globals { protos, ntypes, tables, proto: s.proto } })
}

/// The pointers at `at + off` for each offset.
fn ptrs<const N: usize>(core: &Image, at: usize, offs: [usize; N]) -> Result<[usize; N]> {
    let mut out = [0; N];
    for (p, off) in out.iter_mut().zip(offs) {
        *p = core.ptr_at(at + off)?;
    }
    Ok(out)
}

/// Sets the state and points the lookup call `site` at [`lookup`].
fn commit(cx: &Ctx, globals: Globals, site: usize) -> Result<()> {
    GLOBALS.commit(cx, globals);
    CACHE.set(Cache { slots: vec![Slot::default(); 1 << SLOT_BITS].into(), hits: 0, walks: 0 })?;
    cx.core().retarget(site + 1, lookup as LookupFn)
}

fn print_stats() {
    // SAFETY: exit-time stats.
    let Some(c) = (unsafe { CACHE.shared() }) else { return };
    let mismatches = if verify() { format!(" mismatches={}", verify::MISMATCHES) } else { String::new() };
    eprintln!("dmfast: own proc tables merges={MERGES} remembered={} walked={}{mismatches}", c.hits, c.walks);
}
