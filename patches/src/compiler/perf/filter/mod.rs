//! `filter`: the name-resolution miss filter. Name resolution walks scopes outward and, per scope, indexes declaration
//! children (type 0x1ED in table A, or 0x1F7/0x1F8 in table B) and probes a hash keyed (child, name); most probes
//! miss.
//!
//! Every insert is mirrored as (scope = the child's parent link, name, table) in `found`. When a scope's matching
//! children are all indexed and keyed under it, and `found` lacks the key, every probe would miss, so the child loops
//! are skipped. `found` only grows, so it never under-reports. `deps` guards the insert.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use crate::base::Result;
use crate::base::code::{live_u32, read};
use crate::base::feature::{Ctx, Feature, Hosts, install};
use crate::base::hash;
use crate::base::sync::{Installed, Owned};
use crate::compiler::layout::{CHILDREN, FLAGS, ID, PARENT};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

// Per platform, checked at install: BUCKETS = offset of a table's bucket array pointer (replaced on growth), and
// `has_key(table, child, name)`: whether the table holds that key.
#[cfg(target_os = "linux")]
use linux::{BUCKETS, has_key};
#[cfg(windows)]
use windows::{BUCKETS, has_key};

pub static FILTER: Feature =
    Feature::perf(Hosts::COMPILER, "filter", "name filter", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);

/// Smaller scopes are cheap to walk; checking them costs more than it saves.
const MIN_CHILDREN: u16 = 8;
/// The `var` node id in 516 (see [`crate::compiler::layout`]).
const TYPE_DECL_A: u32 = 0x1ED;
const TYPES_DECL_B: [u32; 2] = [0x1F7, 0x1F8];
const FLAG_INDEXED: u8 = 8;

/// What install found.
struct Found {
    /// 516.1660 (Linux): the table-B loops probe type 0x1ED children against table B too.
    b_has_a: bool,
    tables: [usize; 2],
}

/// The filter, owned by the compiler's thread. The counts are plain `u64`s: they are bumped on every lookup.
struct State {
    /// Bucket array pointers when `complete` was last valid.
    bufs: [u32; 2],
    found: hash::Set<u64>,
    /// (scope | table) -> child count at which all matching children were indexed.
    complete: hash::Map<u32, u16>,
    calls: u64,
    skips: u64,
    inserts: u64,
}

static FOUND: Installed<Found> = Installed::new();
static STATE: Owned<State> = Owned::new();
/// Set once a hook runs on a second thread (its inserts miss `found`): the filter stays off for good.
static OFF: AtomicBool = AtomicBool::new(false);

/// At install, once everything is checked.
fn init(cx: &Ctx, b_has_a: bool, tables: [usize; 2]) -> Result<()> {
    STATE.set(State { bufs: [0; 2], found: hash::set(), complete: hash::map(), calls: 0, skips: 0, inserts: 0 })?;
    FOUND.commit(cx, Found { b_has_a, tables });
    Ok(())
}

/// The filter state, unless the filter is off; a call from a second thread turns it off.
fn state() -> Option<&'static mut State> {
    if OFF.load(Relaxed) {
        return None;
    }
    let st = STATE.get();
    if st.is_none() {
        OFF.store(true, Relaxed);
    }
    st
}

fn key(scope: u32, name: u32, t: u32) -> u64 {
    (u64::from(scope) << 32) | u64::from(name | (t << 31))
}

/// An insert of (child, name) into table `t`.
extern "C" fn record(t: u32, child: u32, name: u32) {
    let Some(st) = state() else { return };
    // SAFETY: `child` is the node BYOND is inserting: live.
    let scope = unsafe { live_u32(child as usize + PARENT) };
    st.found.insert(key(scope, name, t));
    st.inserts += 1;
}

/// At the resolution loop entry: 1 if the child loops of `scope` would find nothing.
extern "C" fn should_skip(scope: usize, name: u32, sel: u32, count: u32) -> u32 {
    let Some(st) = state() else { return 0 };
    let x = FOUND.get();
    st.calls += 1;
    let count = count as u16;
    // SAFETY: the live tables and BYOND's live scope node with its children (the entry stub passes them).
    unsafe {
        let bufs = x.tables.map(|t| live_u32(t + BUCKETS));
        if bufs != st.bufs {
            st.bufs = bufs;
            st.complete.clear();
        }
        let t = u32::from(sel as u8 != 0);
        let matching = |child: usize| {
            let ty = live_u32(child + ID);
            if t == 0 { ty == TYPE_DECL_A } else { TYPES_DECL_B.contains(&ty) || (x.b_has_a && ty == TYPE_DECL_A) }
        };
        let children = live_u32(scope + CHILDREN) as usize;
        let child = |i: usize| live_u32(children + 4 * i) as usize;
        let ck = scope as u32 | t;
        if st.complete.get(&ck) != Some(&count) {
            // Every matching child indexed, and keyed under this scope (its parent link) in `found`.
            for c in (0..usize::from(count)).map(child) {
                if matching(c) && (read::<u8>(c + FLAGS) & FLAG_INDEXED == 0 || live_u32(c + PARENT) as usize != scope)
                {
                    return 0;
                }
            }
            st.complete.insert(ck, count);
        }
        if st.found.contains(&key(scope as u32, name, t)) {
            return 0;
        }
        st.skips += 1;
        let table = x.tables[t as usize];
        if verify::holds(|| (0..usize::from(count)).map(child).any(|c| matching(c) && has_key(table, c as u32, name))) {
            return 0;
        }
    }
    1
}

fn print_stats() {
    // SAFETY: exit-time stats: no hook runs any more.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: name filter calls={} skips={} inserts={} found={} complete={} off={} verify={} mismatches={}",
            st.calls,
            st.skips,
            st.inserts,
            st.found.len(),
            st.complete.len(),
            OFF.load(Relaxed),
            crate::base::driver::verify(),
            verify::MISMATCHES
        );
    }
}
