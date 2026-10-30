//! `varread`'s cache and the hook that answers from it.
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use super::verify;
use super::windows::Calls;
use crate::base::code::live_u32;
use crate::base::sync::Owned;

/// A DM value: type tag (low byte meaningful) and data.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Value {
    pub(super) tag: u32,
    pub(super) data: u32,
}

/// An object kind read inline: `id < [count]`, object = `[table][id]`, own vars at `object + own`, and its type
/// field at `object + key`.
#[derive(Clone, Copy)]
pub(super) struct Kind {
    pub(super) count: usize,
    pub(super) table: usize,
    pub(super) own: usize,
    pub(super) key: usize,
}

/// What install found.
pub(super) struct Found {
    /// The inline kind of each value tag.
    pub(super) kinds: [Option<Kind>; 256],
    /// Globals holding the type table's pointer and count (the cache is for one table).
    pub(super) protos: usize,
    pub(super) nprotos: usize,
    /// The global holding the statics table (a pointer to an array of values).
    pub(super) statics: usize,
    pub(super) calls: Calls,
}

#[derive(Default)]
pub(super) struct Counters {
    hits: u64,
    fills: u64,
    flushes: u64,
    pub(super) checked: u64,
    pub(super) mismatches: u64,
}

pub(super) struct State {
    found: Found,
    /// The type table's pointer and count the cache was filled under.
    protos: u32,
    nprotos: u32,
    cache: Box<[Entry; SIZE]>,
    pub(super) counters: Counters,
}

/// Owned by the first thread to read a var; other threads use BYOND's read.
pub(super) static STATE: Owned<State> = Owned::new();
/// Set when a .dmb load starts (on any thread): the owner empties the cache on its next read.
static STALE: AtomicBool = AtomicBool::new(true);

const BITS: u32 = 14;
const SIZE: usize = 1 << BITS;
/// A type's answer for one var: `name | tag << 24` and the type key, then the initial value or, for a static var,
/// ([`STATIC`], its index).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Entry {
    name: u32,
    key: u32,
    value: Value,
}

/// The key of an empty entry: never cached.
const NO_KEY: u32 = u32::MAX;
const EMPTY: Entry = Entry { name: 0, key: NO_KEY, value: Value { tag: 0, data: 0 } };
/// The tag of a static var's entry (initial values keep only their type byte).
const STATIC: u32 = u32::MAX;

fn hash(name: u32, key: u32) -> usize {
    ((name ^ key.rotate_left(13)).wrapping_mul(0x9E37_79B1) >> (32 - BITS)) as usize
}

/// The hook: the value of var `name` of the value (`tag`, `id`) into `out`, or false for the original read.
///
/// # Safety
/// Only from the read's entry stub, with BYOND's arguments; `out` is writable.
pub(super) unsafe extern "C" fn read(tag: u32, id: u32, name: u32, out: *mut Value) -> bool {
    let Some(st) = STATE.get() else { return false };
    // SAFETY: the globals and functions were checked at install; BYOND's arguments (caller).
    let Some(v) = (unsafe { st.answer(tag, id, name & 0xFF_FFFF) }) else { return false };
    // SAFETY: as above.
    unsafe {
        out.write(v);
        verify::check(st, tag, id, name, v);
    }
    true
}

impl State {
    pub(super) fn new(found: Found) -> State {
        let cache = vec![EMPTY; SIZE].into_boxed_slice().try_into().unwrap_or_else(|_| unreachable!());
        State { found, protos: 0, nprotos: 0, cache, counters: Counters::default() }
    }

    #[inline(always)]
    unsafe fn answer(&mut self, tag: u32, id: u32, name: u32) -> Option<Value> {
        unsafe {
            let Some(kind) = self.found.kinds[(tag & 0xFF) as usize] else { return self.other(tag, id, name) };
            if id >= live_u32(kind.count) {
                return None;
            }
            let obj = live_u32(live_u32(kind.table) as usize + 4 * id as usize) as usize;
            if obj == 0 {
                return None;
            }
            if let Some(v) = search(obj + kind.own, name) {
                return Some(v);
            }
            self.cached(tag, id, name, live_u32(obj + kind.key), None)
        }
    }

    /// Any other value (turfs, clients, images, ...): BYOND's own-var lookup and type; the cache is keyed by the
    /// type.
    #[inline(never)]
    unsafe fn other(&mut self, tag: u32, id: u32, name: u32) -> Option<Value> {
        unsafe {
            let list = self.found.calls.own_list(tag, id);
            if list != 0
                && let Some(v) = search(list, name)
            {
                return Some(v);
            }
            let t = self.found.calls.type_id(tag, id);
            self.cached(tag, id, name, t, Some(t))
        }
    }

    /// The type's answer for (tag, key, name) from the cache, else [`State::fill`]. `t`: the type, if already
    /// computed.
    #[inline(always)]
    unsafe fn cached(&mut self, tag: u32, id: u32, name: u32, key: u32, t: Option<u32>) -> Option<Value> {
        if key == NO_KEY {
            return None;
        }
        let name = name | (tag & 0xFF) << 24;
        // SAFETY: the globals were checked at install.
        let (protos, nprotos) = unsafe { (live_u32(self.found.protos), live_u32(self.found.nprotos)) };
        if STALE.load(Relaxed) || protos != self.protos || nprotos != self.nprotos {
            return unsafe { self.refill(tag, id, name, key, t) };
        }
        let e = self.cache[hash(name, key)];
        if e.name != name || e.key != key {
            return unsafe { self.fill(tag, id, name, key, t) };
        }
        self.counters.hits += 1;
        // SAFETY: as above.
        Some(if e.value.tag == STATIC { unsafe { static_value(&self.found, e.value.data) } } else { e.value })
    }

    /// A .dmb load started, or the type table moved or changed size: empty the cache.
    #[cold]
    #[inline(never)]
    unsafe fn refill(&mut self, tag: u32, id: u32, name: u32, key: u32, t: Option<u32>) -> Option<Value> {
        STALE.store(false, Relaxed);
        self.cache.fill(EMPTY);
        // SAFETY: the globals were checked at install.
        unsafe {
            self.protos = live_u32(self.found.protos);
            self.nprotos = live_u32(self.found.nprotos);
        }
        self.counters.flushes += 1;
        unsafe { self.fill(tag, id, name, key, t) }
    }

    /// Cache miss: BYOND's searches, in its order. `name` has the tag in its top byte.
    #[cold]
    #[inline(never)]
    unsafe fn fill(&mut self, tag: u32, id: u32, name: u32, key: u32, t: Option<u32>) -> Option<Value> {
        unsafe {
            let c = &self.found.calls;
            let t = t.unwrap_or_else(|| c.type_id(tag, id));
            let var = name & 0xFF_FFFF;
            let (value, v) = match c.find_init(t, var) {
                // BYOND copies the type byte only.
                Some(v) => {
                    let v = Value { tag: v.tag & 0xFF, data: v.data };
                    (v, v)
                }
                // Undefined, or not static: BYOND's read handles it (and raises the error).
                None => {
                    let index = c.find_static(t, var)?;
                    (Value { tag: STATIC, data: index }, static_value(&self.found, index))
                }
            };
            self.cache[hash(name, key)] = Entry { name, key, value };
            self.counters.fills += 1;
            Some(v)
        }
    }
}

/// BYOND's search of an own-var list (`{entries, u16 count}`, 16-byte entries {def, name, value} sorted by name).
#[inline(always)]
unsafe fn search(list: usize, name: u32) -> Option<Value> {
    unsafe {
        let mut hi = live_u32(list + 4) as u16 as usize;
        let entries = live_u32(list) as usize;
        let mut lo = 0;
        while lo < hi {
            let mid = (lo + hi) >> 1;
            let e = entries + 16 * mid;
            let n = live_u32(e + 4);
            if n == name {
                return Some(Value { tag: live_u32(e + 8), data: live_u32(e + 12) });
            }
            if name < n {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        None
    }
}

/// Static var `index` from the statics table.
unsafe fn static_value(f: &Found, index: u32) -> Value {
    unsafe {
        let v = live_u32(f.statics) as usize + 8 * index as usize;
        Value { tag: live_u32(v), data: live_u32(v + 4) }
    }
}

/// A .dmb load is starting: nothing cached may outlive the world it came from.
pub(super) extern "C" fn flush() {
    STALE.store(true, Relaxed);
}

pub(super) fn print_stats() {
    // SAFETY: exit-time stats; the owner no longer runs hooks.
    if let Some(st) = unsafe { STATE.shared() } {
        let c = &st.counters;
        eprintln!(
            "dmfast: varread hits={} fills={} flushes={} checked={} mismatches={}",
            c.hits, c.fills, c.flushes, c.checked, c.mismatches
        );
    }
}
