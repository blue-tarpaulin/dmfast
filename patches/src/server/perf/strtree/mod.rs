//! `strtree`: the runtime string table's lookups from a hash index instead of its tree.
//!
//! The table is also an unbalanced binary search tree (entry 0 is the root), walked one string compare per level by
//! every lookup: interning a string, the .dmb loader, `find`, and removal. Non-empty strings are unique among linked
//! entries (an empty one always matches the root), so the answer doesn't depend on the tree's shape, and a hash index
//! gives the same one. A miss hands out [`CELL`] as the empty link; its caller stores the new entry there before any
//! other operation, which indexes it. Removal, the only reader of the links, is taken over too, so the real links go
//! stale: BYOND's rebuilds stop after zeroing them. An indexed entry is used only while it is still its id's entry,
//! and the index is cleared when the table empties, its root changes, or a .dmb load begins. `DMFAST_VERIFY=1` keeps
//! the real tree and checks every answer against it.
//!
//! Dependency guards (`deps`): the lookup, removal, intern, find, the rebuild and its tree builder, and the .dmb
//! string table load.

use std::ffi::CStr;
use std::hash::Hasher;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::{Foreign, live_u32, read};
use crate::base::driver::verify;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::hash::{self, FxHasher};
use crate::base::sync::{Installed, lock};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static STRTREE: Feature =
    Feature::perf(Hosts::SERVER, "strtree", "string tree", install!(windows: windows::install, linux: linux::install))
        .stats(print_stats);

/// The lookup (cdecl: &string, &link -> entry or 0; on a miss, the link is where the caller stores its new entry).
type LookupFn = unsafe extern "C" fn(*const *const u8, *mut usize) -> usize;

/// A table entry: its string, id (its index in the table), and tree links; the .dmb loader also sets its reference
/// count, CRC, and length.
const STRING: usize = 0;
const ID: usize = 4;
const LEFT: usize = 8;
const RIGHT: usize = 0xC;
#[cfg(windows)]
const REFS: usize = 0x10;
#[cfg(windows)]
const CRC: usize = 0x14;
#[cfg(windows)]
const LENGTH: usize = 0x18;

/// What install found.
struct Found {
    /// The globals holding the entry array and the entry count.
    table: usize,
    count: usize,
    /// Enables BYOND's rebuild on a long walk (anything but -1).
    checksum: usize,
    /// The flag byte BYOND sets while the .dmb loader links the table's entries.
    loading: usize,
    /// The original lookup (verify mode: its rebuild path).
    original: Foreign<LookupFn>,
}

static FOUND: Installed<Found> = Installed::new();

/// What `strtable` needs: the table's globals, and the lookup and the rebuild that the load pass calls.
#[cfg(windows)]
pub(crate) struct Sites {
    pub(crate) table: usize,
    pub(crate) count: usize,
    pub(crate) loading: usize,
    pub(crate) lookup: usize,
    pub(crate) rebuild: usize,
}

#[cfg(windows)]
pub(crate) static SITES: Installed<Sites> = Installed::new();

/// The empty link a miss hands out. One suffices: BYOND's intern, unlocked, serializes its callers.
static CELL: AtomicU32 = AtomicU32::new(0);
/// The link a hit hands out (none of the callers left writes it).
static HIT_LINK: AtomicU32 = AtomicU32::new(0);

/// The index, for every thread that looks up.
struct State {
    /// Entry 0 at the last operation (0: the table was empty); `None` before the first.
    root: Option<usize>,
    /// The .dmb loader's flag at the last operation (a reload can reuse the root's address).
    was_loading: bool,
    /// String hash -> entry; a string whose hash another indexed string holds goes to `collided`.
    index: hash::Map<u32, usize>,
    collided: hash::Map<Box<[u8]>, usize>,
    /// Verify mode: the real empty link the last miss handed out.
    pending: Option<usize>,
    /// Verify mode: the real tree may hold more than the index (a .dmb load is linking it).
    stale: bool,
    hits: u64,
    misses: u64,
    removes: u64,
    resets: u64,
    mismatches: u64,
    /// Hits that were no longer their id's entry (see [`State::find`]).
    dropped: u64,
}

impl State {
    const fn new() -> State {
        State {
            root: None,
            was_loading: false,
            index: hash::map(),
            collided: hash::map(),
            pending: None,
            stale: false,
            hits: 0,
            misses: 0,
            removes: 0,
            resets: 0,
            mismatches: 0,
            dropped: 0,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

/// The string of entry `e`.
///
/// # Safety
/// `e` is a live entry.
unsafe fn text<'a>(e: usize) -> &'a [u8] {
    // SAFETY: a live entry (caller): its string.
    unsafe { CStr::from_ptr(live_u32(e + STRING) as *const _).to_bytes() }
}

/// 32-bit string hash (halves the index's slot size on x86); collisions go to `collided`.
fn hash(s: &[u8]) -> u32 {
    let mut h = FxHasher::default();
    h.write(s);
    h.finish() as u32
}

/// Whether `e` is still the table's entry for its id.
fn live(e: usize) -> bool {
    let f = FOUND.get();
    // SAFETY: the table's globals (install), and an entry once indexed, whose id field stays mapped.
    unsafe {
        let (table, count) = (live_u32(f.table) as usize, live_u32(f.count) as usize);
        let id = live_u32(e + ID) as usize;
        table != 0 && id < count && live_u32(table + 4 * id) as usize == e
    }
}

impl State {
    /// Start of every operation: notice a reloaded table, and index what the last miss's caller stored. Entry 0, or
    /// `None` while the table is empty.
    fn sync(&mut self) -> Option<usize> {
        let f = FOUND.get();
        // SAFETY: the table's globals and the loader's flag (install); entry 0 of a non-empty table.
        let (loaded, root, loading) = unsafe {
            let table = live_u32(f.table) as usize;
            let loaded = table != 0 && live_u32(f.count) != 0;
            (loaded, if loaded { live_u32(table) as usize } else { 0 }, read::<u8>(f.loading) != 0)
        };
        let load_began = loading && !self.was_loading;
        self.was_loading = loading;
        if self.root != Some(root) || load_began {
            self.root = Some(root);
            self.clear();
            self.pending = None;
            CELL.store(0, Relaxed);
            self.resets += 1;
        }
        if verify() {
            self.sync_real(loading);
        } else {
            let stored = CELL.swap(0, Relaxed) as usize;
            if stored != 0 {
                self.insert(stored);
            }
        }
        loaded.then_some(root)
    }

    fn clear(&mut self) {
        self.index.clear();
        self.collided.clear();
    }

    /// The indexed entry for `key`. An entry that is no longer its id's entry is dropped before its string is read:
    /// only the world reset frees entries without removing them, and it empties the table, so `sync` has cleared
    /// the index.
    fn find(&mut self, key: &[u8]) -> Option<usize> {
        let h = hash(key);
        if let Some(&e) = self.index.get(&h) {
            if !live(e) {
                self.index.remove(&h);
                self.dropped += 1;
            // SAFETY: live, checked just above.
            } else if unsafe { text(e) } == key {
                return Some(e);
            }
        }
        let &e = self.collided.get(key)?;
        if live(e) {
            return Some(e);
        }
        self.collided.remove(key);
        self.dropped += 1;
        None
    }

    /// Indexes the live entry `e`.
    fn insert(&mut self, e: usize) {
        // SAFETY: a live entry (caller).
        let key = unsafe { text(e) };
        let h = hash(key);
        match self.index.get(&h) {
            // SAFETY: live, checked first.
            Some(&o) if o != e && live(o) && unsafe { text(o) } != key => {
                self.collided.insert(key.into(), e);
            }
            _ => {
                self.index.insert(h, e);
            }
        }
    }

    /// Drops the live entry `e` if it is the indexed entry for its string.
    fn remove(&mut self, e: usize) -> bool {
        // SAFETY: a live entry (caller).
        let key = unsafe { text(e) };
        let h = hash(key);
        if self.index.get(&h) == Some(&e) {
            self.index.remove(&h);
        } else if self.collided.get(key) == Some(&e) {
            self.collided.remove(key);
        } else {
            return false;
        }
        self.removes += 1;
        true
    }
}

/// Replaces the lookup.
///
/// # Safety
/// BYOND's arguments: the address of a C string, and a writable link.
unsafe extern "C" fn lookup(key: *const *const u8, out: *mut usize) -> usize {
    let mut st = lock(&STATE);
    let Some(root) = st.sync() else {
        // SAFETY: writable (caller).
        unsafe { *out = 0 };
        return 0;
    };
    // SAFETY: a C string (caller); entry 0 of the table, live.
    let (k, is_root) = unsafe {
        let k = CStr::from_ptr(*key as *const _).to_bytes();
        (k, text(root) == k)
    };
    let found = if is_root { Some(root) } else { st.find(k) };
    if found.is_some() {
        st.hits += 1;
    } else {
        st.misses += 1;
    }
    if verify() {
        drop(st);
        // SAFETY: BYOND's arguments (caller).
        return unsafe { verify::lookup(key, out, found) };
    }
    let link = match found {
        Some(e) if e == root => 0,
        Some(_) => HIT_LINK.as_ptr() as usize,
        None => CELL.as_ptr() as usize,
    };
    // SAFETY: writable (caller).
    unsafe { *out = link };
    found.unwrap_or(0)
}

/// Replaces removal's lookup and unlink (fast mode): 1 if `entry` was the indexed entry for its string (go on past
/// the unlink), else 0 (BYOND's not-found path).
extern "C" fn remove(entry: usize) -> u32 {
    let mut st = lock(&STATE);
    st.sync();
    u32::from(st.remove(entry))
}

/// The .dmb loader's pass over the string table (fast mode), in place of BYOND's loop: set each entry's id, clear
/// its links and reference count, index it unless its string is there already, fill in an unset length, and set its
/// CRC, `crc(crc(seed, id, 4), string, length + 1)`. False, with nothing changed, if the table isn't loaded.
///
/// # Safety
/// Called by BYOND's loader with the table loaded; no other thread uses it.
#[cfg(windows)]
pub(crate) unsafe fn load_all(seed: u32) -> bool {
    use crate::base::code::write;
    use crate::server::perf::crc::crc;

    let f = FOUND.get();
    // SAFETY: the table's globals (install), `count` entry pointers in the table, each 0 or a live entry whose
    // string is NUL-terminated and, with its length, readable (as BYOND's loop reads them); no other thread uses them
    // (caller).
    unsafe {
        let (table, count) = (live_u32(f.table) as usize, live_u32(f.count) as usize);
        if table == 0 || count == 0 || live_u32(table) == 0 {
            return false;
        }
        let root = live_u32(table) as usize;
        let mut st = lock(&STATE);
        st.sync();
        // The pass links every entry anew.
        st.clear();
        st.index.reserve(count);
        for i in 0..count {
            prefetch(table, i, count);
            let e = live_u32(table + 4 * i) as usize;
            if e == 0 {
                continue;
            }
            write(e + ID, i as u32);
            write(e + RIGHT, 0u32);
            write(e + LEFT, 0u32);
            let s = text(e);
            if text(root) == s || st.link(e, s) {
                st.hits += 1;
            } else {
                st.misses += 1;
            }
            write(e + REFS, 0u32);
            let mut len = live_u32(e + LENGTH);
            if len == 0 {
                len = s.len() as u32;
                write(e + LENGTH, len);
            }
            let id = i as u32;
            write(e + CRC, crc(crc(seed, (&raw const id).cast(), 4), s.as_ptr(), len + 1));
        }
        true
    }
}

/// Starts loading what [`load_all`] reads a few steps ahead, the entry and then its string: the cache misses
/// dominate the pass.
///
/// # Safety
/// As [`load_all`]'s.
#[cfg(windows)]
#[inline(always)]
unsafe fn prefetch(table: usize, i: usize, count: usize) {
    use std::arch::x86::{_MM_HINT_T0, _mm_prefetch};
    const AHEAD: usize = 16;
    // SAFETY: the table's first `count` pointers (caller); a prefetch never faults.
    unsafe {
        if i + AHEAD < count {
            _mm_prefetch::<_MM_HINT_T0>(live_u32(table + 4 * (i + AHEAD)) as *const i8);
        }
        if i + AHEAD / 2 < count {
            let e = live_u32(table + 4 * (i + AHEAD / 2)) as usize;
            if e != 0 {
                _mm_prefetch::<_MM_HINT_T0>(live_u32(e + STRING) as *const i8);
            }
        }
    }
}

#[cfg(windows)]
impl State {
    /// [`load_all`]'s lookup of the string `s` of entry `e`, which indexes `e` on a miss: whether `s` was there
    /// already. Every indexed entry is from this pass, so live.
    fn link(&mut self, e: usize, s: &[u8]) -> bool {
        use std::collections::hash_map::Entry;
        match self.index.entry(hash(s)) {
            Entry::Vacant(v) => {
                v.insert(e);
                false
            }
            Entry::Occupied(o) => {
                // SAFETY: live (above).
                if unsafe { text(*o.get()) } == s || self.collided.contains_key(s) {
                    return true;
                }
                self.collided.insert(s.into(), e);
                false
            }
        }
    }
}

/// The index as [`load_all`] left it, for `strtable`'s check.
#[cfg(windows)]
pub(crate) struct Index(hash::Map<u32, usize>, hash::Map<Box<[u8]>, usize>);

/// Takes the index, leaving it empty.
#[cfg(windows)]
pub(crate) fn take_index() -> Index {
    let mut st = lock(&STATE);
    st.sync();
    Index(std::mem::take(&mut st.index), std::mem::take(&mut st.collided))
}

/// Whether the index now holds exactly what `index` held.
#[cfg(windows)]
pub(crate) fn index_matches(index: &Index) -> bool {
    let mut st = lock(&STATE);
    st.sync();
    st.index == index.0 && st.collided == index.1
}

fn print_stats() {
    if !FOUND.is_set() {
        return;
    }
    let st = lock(&STATE);
    eprintln!(
        "dmfast: string tree index hits={} misses={} removes={} resets={} collided={} verify={} mismatches={} dropped={}",
        st.hits,
        st.misses,
        st.removes,
        st.resets,
        st.collided.len(),
        verify(),
        st.mismatches,
        st.dropped
    );
}
