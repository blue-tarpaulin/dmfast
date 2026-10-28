//! `listinit`: list-valued var initializers (`var/list/l = list(1, 2)`) are deduplicated through a hash index.
//!
//! The compiler keeps each distinct initializer once in a table, and vars refer to it by index (id). Stock finds an
//! equal entry by scanning the whole table and grows it one slot per append. The replacement looks entries up in a
//! hash index and appends the same way, with spare capacity.
//!
//! An entry is `{pairs, len}`: 8-byte pairs, and the pair count in the low 16 bits of `len`. Entries are equal when
//! every pair has the same first-dword low byte and second dword. Id 0xFFFF is skipped.
//!
//! Only the intern, a getter, and a reset touch the table, so index hits are checked against it. Null and empty
//! initializers go to the original.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use rustc_hash::FxHashMap;

use crate::base::code::{Foreign, MallocFn, ReallocFn, live_u32};
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::lock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static LISTINIT: Feature = Feature::perf(
    Hosts::COMPILER,
    "listinit",
    "list initializer intern",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// The id the compiler skips.
const RESERVED: u32 = 0xFFFF;

/// What install found: the table's globals, the compiler's allocator, and the original intern.
struct Found {
    /// The global holding the table's entry array (an array of entry pointers).
    array: usize,
    /// The global holding the table's entry count.
    count: usize,
    malloc: Foreign<MallocFn>,
    realloc: Foreign<ReallocFn>,
    /// The original intern (cdecl, list -> id).
    original: Foreign<unsafe extern "C" fn(usize) -> u32>,
}

struct State {
    found: Found,
    /// Each interned list's key and id.
    index: FxHashMap<Box<[u64]>, u32>,
    /// The entry array we last allocated and its capacity; a different live array means the compiler replaced it.
    array: u32,
    capacity: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static HITS: AtomicU32 = AtomicU32::new(0);
static INSERTS: AtomicU32 = AtomicU32::new(0);
static STALE: AtomicU32 = AtomicU32::new(0);

/// The pair count of `list`.
unsafe fn len(list: usize) -> usize {
    unsafe { live_u32(list + 4) as u16 as usize }
}

/// The parts of `list`'s pairs that equality compares: the first dword's low byte and the second dword.
unsafe fn key(list: usize) -> Box<[u64]> {
    unsafe {
        let pairs = live_u32(list) as usize;
        (0..len(list))
            .map(|i| (u64::from(live_u32(pairs + 8 * i) as u8) << 32) | u64::from(live_u32(pairs + 8 * i + 4)))
            .collect()
    }
}

unsafe fn put(addr: usize, v: u32) {
    unsafe { (addr as *mut u32).write(v) }
}

/// Whether the table still holds a list with key `want` at `id`.
unsafe fn valid(found: &Found, want: &[u64], id: u32) -> bool {
    unsafe {
        if id == RESERVED || id >= live_u32(found.count) {
            return false;
        }
        let entry = live_u32(live_u32(found.array) as usize + 4 * id as usize) as usize;
        entry != 0 && len(entry) == want.len() && *key(entry) == *want
    }
}

/// Append a copy of `list` as stock does, but with spare capacity. Returns its id.
unsafe fn append(st: &mut State, list: usize) -> u32 {
    unsafe {
        let found = &st.found;
        let old = live_u32(found.count);
        let (count, id) = if old == RESERVED { (RESERVED + 2, RESERVED + 1) } else { (old + 1, old) };
        let array = live_u32(found.array);
        if array != st.array || count > st.capacity {
            let grown = if array == st.array { st.capacity * 2 } else { 0 };
            let capacity = count.max(grown).max(64);
            let a = found.realloc.get()(array as usize as *mut _, capacity as usize * 4) as u32;
            put(found.array, a);
            st.array = a;
            st.capacity = capacity;
        }
        put(found.count, count);
        let n = len(list);
        let entry = found.malloc.get()(8) as usize;
        let pairs = found.malloc.get()(n * 8) as usize;
        if n > 0 {
            std::ptr::copy_nonoverlapping(live_u32(list) as *const u8, pairs as *mut u8, n * 8);
        }
        put(entry, pairs as u32);
        put(entry + 4, live_u32(list + 4));
        put(st.array as usize + 4 * id as usize, entry as u32);
        id
    }
}

fn init(found: Found) {
    *lock(&STATE) = Some(State { found, index: FxHashMap::default(), array: 0, capacity: 0 });
}

/// Replaces the list initializer intern (cdecl, initializer -> id).
extern "C" fn intern(list: usize) -> u32 {
    // SAFETY: `list` is 0 or the compiler's initializer; the globals and functions were checked at install.
    unsafe {
        let mut guard = lock(&STATE);
        let st = guard.as_mut().expect("list initializer intern called before init");
        if list == 0 || len(list) == 0 {
            // The original appends these without a lookup and reallocates the array to exactly its count.
            let id = st.found.original.get()(list);
            st.array = live_u32(st.found.array);
            st.capacity = live_u32(st.found.count);
            return id;
        }
        let key = key(list);
        if let Some(&id) = st.index.get(&key) {
            if valid(&st.found, &key, id) {
                HITS.fetch_add(1, Relaxed);
                return id;
            }
            STALE.fetch_add(1, Relaxed);
        }
        INSERTS.fetch_add(1, Relaxed);
        let id = append(st, list);
        st.index.insert(key, id);
        id
    }
}

fn print_stats() {
    eprintln!(
        "dmfast: list initializer intern hits={} inserts={} stale={}",
        HITS.load(Relaxed),
        INSERTS.load(Relaxed),
        STALE.load(Relaxed)
    );
}
