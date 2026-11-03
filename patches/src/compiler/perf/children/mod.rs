//! `children`: the child index. Path resolution asks for "the n-th child named X" by scanning a type node's
//! children; for nodes with at least [`MIN_CHILDREN`] children, a per-node index from name to positions answers it.
//!
//! Children are only appended, so an index is extended in place, and rebuilt when the count shrinks or the first or
//! last indexed child changes (address reuse). Hits are checked against the live array; misses are not, which is safe
//! because only the child-array append writes the array, and names are set by then. Small nodes, other threads, and
//! anything undecidable run the original loop. `deps` guards the scan.

use crate::base::feature::{Feature, Hosts, install};
use crate::base::hash::Map;
use crate::base::sync::Owned;
use crate::base::{Result, code::read};
use crate::compiler::layout::{CHILDREN, COUNT, ID};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static CHILD_INDEX: Feature = Feature::perf(
    Hosts::COMPILER,
    "children",
    "child index",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

const MIN_CHILDREN: u16 = 32;
/// [`lookup`]'s answer for the stubs: run the original scan.
const FALLBACK: usize = 1;

/// A node's index of its first `indexed` children.
#[derive(Default)]
struct NodeIndex {
    first: u32,
    last: u32,
    indexed: u16,
    /// Each name's child positions, in order.
    by_name: Map<u32, Vec<u16>>,
}

/// Owned by the compiler's thread; the counts are plain adds on the hot path, `u64` like every lookup count.
struct State {
    nodes: Map<u32, NodeIndex>,
    hits: u64,
    misses: u64,
    fallbacks: u64,
    rebuilds: u64,
}

static STATE: Owned<State> = Owned::new();

/// Set the state (the stubs' install).
fn init() -> Result<()> {
    STATE.set(State { nodes: Map::default(), hits: 0, misses: 0, fallbacks: 0, rebuilds: 0 })
}

/// [`lookup`] for `node`'s children `kids` (at least [`MIN_CHILDREN`]): the child, 0 for none, or `None` to fall back.
fn indexed_lookup(
    st: &mut State,
    node: usize,
    kids: &[u32],
    name_of: impl Fn(usize) -> u32,
    name: u32,
    n: u16,
) -> Option<usize> {
    let count = kids.len() as u16;
    let e = st.nodes.entry(node as u32).or_default();
    let stale = count < e.indexed || (e.indexed > 0 && (kids[0] != e.first || kids[e.indexed as usize - 1] != e.last));
    if stale {
        st.rebuilds += 1;
        e.by_name.clear();
        e.indexed = 0;
    }
    // An unset slot: fall back before indexing any of the new children, so a retry doesn't index them twice.
    let new = &kids[e.indexed as usize..];
    if new.contains(&0) {
        return None;
    }
    for (i, &c) in new.iter().enumerate() {
        e.by_name.entry(name_of(c as usize)).or_default().push(e.indexed + i as u16);
    }
    (e.indexed, e.first, e.last) = (count, kids[0], kids[kids.len() - 1]);
    let Some(&pos) = e.by_name.get(&name).and_then(|v| v.get(n as usize)) else { return Some(0) };
    // Indexed positions are below `count`.
    let c = kids[pos as usize];
    (name_of(c as usize) == name).then_some(c as usize)
}

/// The n-th (0-based) child of `node` named `name`: the child, 0 for a certain miss, or [`FALLBACK`]. Called by the
/// scan stubs.
///
/// # Safety
/// `node` is a live type node and its children live nodes (the compiler's tree, on its thread).
unsafe extern "C" fn lookup(node: usize, name: u32, n: u32) -> usize {
    let Some(st) = STATE.get() else { return FALLBACK };
    // SAFETY: a live node (caller): its child count and array.
    let kids = unsafe {
        let count = read::<u16>(node + COUNT);
        if count < MIN_CHILDREN {
            return FALLBACK;
        }
        std::slice::from_raw_parts(read::<u32>(node + CHILDREN) as *const u32, count as usize)
    };
    // SAFETY: a nonzero child of a live node is a live node (caller); zero ones are never read.
    let name_of = |c: usize| unsafe { read::<u32>(c + ID) };
    let n = n as u16;
    let Some(r) = indexed_lookup(st, node, kids, name_of, name, n) else {
        st.fallbacks += 1;
        return FALLBACK;
    };
    if !verify::agrees(r, || verify::scan(kids, name_of, name, n)) {
        return FALLBACK;
    }
    if r == 0 {
        st.misses += 1;
    } else {
        st.hits += 1;
    }
    r
}

fn print_stats() {
    // SAFETY: exit-time stats: the compiler thread no longer runs the hook.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: child index nodes={} hits={} misses={} fallbacks={} rebuilds={} verify={} mismatches={}",
            st.nodes.len(),
            st.hits,
            st.misses,
            st.fallbacks,
            st.rebuilds,
            crate::base::driver::verify(),
            verify::MISMATCHES
        );
    }
}
