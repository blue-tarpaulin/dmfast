//! `procs`: BYOND's find-or-add over the proc table (44-byte records) answered from a hash index, not a linear scan.
//!
//! The key is exactly what BYOND's comparator checks (dwords at +0/4/8/0Ch/14h/1Ch/20h/24h, bytes at +18h/19h/1Ah),
//! matched by signature. It's safe because a hit is returned only if the live entry still has that key (hash clashes
//! go to `collided`), and the index is rebuilt when the array pointer or count change, or an indexed entry's hash
//! changed. BYOND edits one key field in place, the path string at +0 (callers on the returned entry, and the .dmb
//! loader on pathless init procs), so pathless entries are also indexed by their other fields. A key the index doesn't
//! know is not in the array and goes to BYOND's append code. `DMFAST_VERIFY=1` also scans, and counts disagreements.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHasher};

use crate::base::Result;
use crate::base::code::{Foreign, live_u32};
use crate::base::driver::Counter;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Owned;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static PROCS: Feature = Feature::perf(
    Hosts::SERVER,
    "procs",
    "proc-table index",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

const ENTRY: usize = 0x2C;
const DWORDS: [usize; 8] = [0, 4, 8, 0xC, 0x14, 0x1C, 0x20, 0x24];
const BYTES: [usize; 3] = [0x18, 0x19, 0x1A];
/// What BYOND returns when the array cannot grow (also a valid index; ambiguous in the original).
const FAILED: u32 = 0xFFFF;
/// The path string id (+0) of an entry that has none yet.
const NO_PATH: u32 = 0xFFFF;

/// A find-or-add entry point (cdecl: key record -> index).
type FindFn = unsafe extern "C" fn(usize) -> u32;

/// What install found.
struct Globals {
    /// Addresses of BYOND's count and array-pointer globals.
    count: usize,
    array: usize,
    /// The whole original function, and its append code entered as if the scan found nothing.
    original: usize,
    append: usize,
}

static ORIGINAL: Foreign<FindFn> = Foreign::new();
static APPEND: Foreign<FindFn> = Foreign::new();
static STATE: Owned<State> = Owned::new();
static ADDS: Counter = Counter::new();
static REBUILDS: Counter = Counter::new();
/// Lookups answered by an entry found through `unnamed`.
static RENAMED: Counter = Counter::new();

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key([u32; 8], [u8; 3]);

/// # Safety
/// `rec` is a live 44-byte record.
unsafe fn key_of(rec: usize) -> Key {
    // SAFETY: a live record (caller).
    unsafe { Key(DWORDS.map(|o| live_u32(rec + o)), BYTES.map(|o| *((rec + o) as *const u8))) }
}

fn hash(k: &Key) -> u32 {
    let mut h = FxHasher::default();
    k.hash(&mut h);
    h.finish() as u32
}

/// The hash of every key field but the path string.
fn pathless_hash(k: &Key) -> u32 {
    let mut k = *k;
    k.0[0] = 0;
    hash(&k)
}

fn entry(array: u32, i: u32) -> usize {
    array as usize + ENTRY * i as usize
}

struct State {
    /// Addresses of BYOND's count and array-pointer globals.
    count_at: usize,
    array_at: usize,
    /// Array pointer and count the index matches.
    array: u32,
    count: u32,
    /// Key hash -> first index with that key.
    index: FxHashMap<u32, u32>,
    /// Keys whose hash `index` holds for a different key -> first index.
    collided: FxHashMap<Key, u32>,
    /// Entries just returned, with their indexed key: callers often set the path at +0, so the next lookup re-keys.
    fresh: Vec<(u32, Key)>,
    /// (`pathless_hash`, index) of entries indexed without a path, sorted: BYOND may set it in place.
    unnamed: Vec<(u32, u32)>,
    /// Lookups answered from the index (`u64`: one per proc reference resolved, so it can pass 2^32).
    hits: u64,
}

/// An entry the index points at changed behind it: rebuild.
struct Stale;

impl State {
    /// BYOND's array pointer and count now.
    fn globals(&self) -> (u32, u32) {
        // SAFETY: BYOND's array and count globals, found at install (hooks run after it).
        unsafe { (live_u32(self.array_at), live_u32(self.count_at)) }
    }

    /// Records `i` for `k`, keeping the first index per key. `array` holds the indexed entries.
    unsafe fn add(&mut self, array: u32, k: Key, i: u32) {
        let h = hash(&k);
        match self.index.get_mut(&h) {
            None => {
                self.index.insert(h, i);
            }
            Some(j) if unsafe { key_of(entry(array, *j)) } == k => *j = (*j).min(i),
            Some(_) => {
                self.collided.entry(k).and_modify(|j| *j = (*j).min(i)).or_insert(i);
            }
        }
    }

    /// Forgets `i` as the index for `k`, if it is.
    fn drop_key(&mut self, k: &Key, i: u32) {
        let h = hash(k);
        if self.index.get(&h) == Some(&i) {
            self.index.remove(&h);
        } else if self.collided.get(k) == Some(&i) {
            self.collided.remove(k);
        }
    }

    unsafe fn rebuild(&mut self, array: u32, count: u32) {
        REBUILDS.bump();
        self.fresh.clear();
        self.index.clear();
        self.collided.clear();
        self.unnamed.clear();
        self.index.reserve(count as usize);
        for i in 0..count {
            let k = unsafe { key_of(entry(array, i)) };
            unsafe { self.add(array, k, i) };
            if k.0[0] == NO_PATH {
                self.unnamed.push((pathless_hash(&k), i));
            }
        }
        self.unnamed.sort_unstable();
        (self.array, self.count) = (array, count);
    }

    /// Moves each fresh entry to the key it holds now (keeping the first index per key).
    unsafe fn rekey_fresh(&mut self, array: u32, count: u32) {
        for (i, was) in std::mem::take(&mut self.fresh) {
            if i >= count {
                continue;
            }
            let now = unsafe { key_of(entry(array, i)) };
            if now != was {
                self.drop_key(&was, i);
                unsafe { self.add(array, now, i) };
            }
            let e = (pathless_hash(&now), i);
            if let (NO_PATH, Err(at)) = (now.0[0], self.unnamed.binary_search(&e)) {
                self.unnamed.insert(at, e);
            }
        }
    }

    /// The first entry indexed without a path that now has key `k`.
    unsafe fn renamed(&self, array: u32, count: u32, k: Key) -> Option<u32> {
        let h = pathless_hash(&k);
        let from = self.unnamed.partition_point(|&(x, _)| x < h);
        self.unnamed[from..]
            .iter()
            .take_while(|&&(x, _)| x == h)
            .map(|&(_, i)| i)
            .find(|&i| i < count && unsafe { key_of(entry(array, i)) } == k)
    }

    /// The index's answer for `k` without rebuilding: the entry, or `None` if `k` isn't in the array.
    unsafe fn lookup(&self, array: u32, count: u32, k: Key) -> Result<Option<u32>, Stale> {
        let live = |i: u32| i < count && unsafe { key_of(entry(array, i)) } == k;
        let h = hash(&k);
        if let Some(&i) = self.index.get(&h) {
            if live(i) {
                return Ok(Some(i));
            }
            // Another key filed under this hash (a collision) is fine; anything else went stale.
            if i >= count || hash(&unsafe { key_of(entry(array, i)) }) != h {
                return Err(Stale);
            }
        }
        match self.collided.get(&k) {
            Some(&i) if live(i) => Ok(Some(i)),
            Some(_) => Err(Stale),
            None => Ok(None),
        }
    }

    /// The index's answer, rebuilding first if the array changed behind it (or a hit went stale).
    unsafe fn find(&mut self, array: u32, count: u32, k: Key) -> Option<u32> {
        if (array, count) != (self.array, self.count) {
            unsafe { self.rebuild(array, count) };
        } else if !self.fresh.is_empty() {
            unsafe { self.rekey_fresh(array, count) };
        }
        let found = match unsafe { self.lookup(array, count, k) } {
            Ok(found) => found,
            Err(Stale) => {
                unsafe { self.rebuild(array, count) };
                unsafe { self.lookup(array, count, k) }.unwrap_or(None)
            }
        };
        match unsafe { self.renamed(array, count, k) } {
            Some(r) if found.is_none_or(|f| r < f) => {
                RENAMED.bump();
                Some(r)
            }
            _ => found,
        }
    }
}

/// Replaces the find-or-add.
///
/// # Safety
/// `key` is a live key record (BYOND passes it).
unsafe extern "C" fn find_or_add(key: usize) -> u32 {
    // SAFETY: a live key (caller); the array holds `count` live records; ORIGINAL and APPEND enter BYOND's
    // find-or-add, set at install.
    unsafe {
        let Some(st) = STATE.get() else { return ORIGINAL.get()(key) };
        let k = key_of(key);
        let (array, count) = st.globals();
        let found = st.find(array, count, k);
        if verify::disagrees(array, count, k, found) {
            st.count = u32::MAX; // rebuild next time
            return ORIGINAL.get()(key);
        }
        if let Some(i) = found {
            st.hits += 1;
            st.fresh.push((i, k));
            return i;
        }
        ADDS.bump();
        // The append runs BYOND code; take the state again afterwards.
        let i = APPEND.get()(key);
        let Some(st) = STATE.get() else { return i };
        let (array, count) = st.globals();
        if i != FAILED && i < count && count == st.count.wrapping_add(1) {
            st.add(array, k, i);
            st.fresh.push((i, k));
            (st.array, st.count) = (array, count);
        } else {
            st.count = u32::MAX;
        }
        i
    }
}

/// Install time.
fn init(g: Globals) -> Result<()> {
    ORIGINAL.set(g.original);
    APPEND.set(g.append);
    STATE.set(State {
        count_at: g.count,
        array_at: g.array,
        array: 0,
        count: u32::MAX,
        index: FxHashMap::default(),
        collided: FxHashMap::default(),
        fresh: Vec::new(),
        unnamed: Vec::new(),
        hits: 0,
    })
}

fn print_stats() {
    // SAFETY: exit-time stats: the server no longer runs the hook.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: proc-table index hits={} (renamed {RENAMED}) adds={ADDS} rebuilds={REBUILDS} verify={} mismatches={}",
            st.hits,
            crate::base::driver::verify(),
            verify::MISMATCHES
        );
    }
}
