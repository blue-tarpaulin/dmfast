//! `sortmap`: BYOND's sorted map insert with geometric growth, and its map merge in one pass.
//!
//! BYOND keeps small maps as sorted arrays of 12-byte entries (`{entries*, u16 count}`, keyed by the dword at +0),
//! reallocates to exactly count+1 on every insert, and merges entry by entry. The insert here grows to 1.125x, only
//! when the block's usable size is short, which is safe because nothing reads the block's size. [`merge_into`] builds
//! the same map as BYOND's merge, calling BYOND's own functions ([`Merge`]) in the same order, and leaves the cases
//! it doesn't handle to BYOND. `DMFAST_VERIFY=1` checks each merged map against the union BYOND's merge produces.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::Foreign;
use crate::base::driver::Counter;
use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static SORTMAP: Feature = Feature::perf(
    Hosts::SERVER,
    "sortmap",
    "sorted maps",
    install!(windows: windows::install_all, linux: linux::install_all),
)
.stats(print_stats);

const ENTRY: usize = 12;
/// How many source entries ahead the merge prefetches value array headers.
const PREFETCH: usize = 8;

#[repr(C)]
struct Map {
    entries: *mut u8,
    count: u16,
}

/// The allocator BYOND uses for the arrays, its usable-size query, and BYOND's out-of-memory handler.
struct Alloc {
    realloc: Foreign<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    usable: Foreign<unsafe extern "C" fn(*mut c_void) -> usize>,
    out_of_memory: Foreign<unsafe extern "C" fn()>,
}

/// The functions BYOND's merge calls; value arrays are shared dword arrays with a `[size, refcount]` header.
struct Merge {
    malloc: Foreign<unsafe extern "C" fn(usize) -> *mut c_void>,
    free: Foreign<unsafe extern "C" fn(*mut c_void)>,
    /// `(array) -> array`: adds a reference.
    copy_shared: Foreign<unsafe extern "C" fn(u32) -> u32>,
    /// `(array) -> u8`: drops a reference, freeing the array at zero.
    free_shared: Foreign<unsafe extern "C" fn(u32) -> u8>,
    /// `(bytes) -> array`: a new array of `bytes` bytes with one reference, or 0.
    alloc_shared: Foreign<unsafe extern "C" fn(usize) -> u32>,
    /// `(id) -> proc`: the proc with this id, or null.
    proc_at: Foreign<unsafe extern "C" fn(u32) -> usize>,
}

/// Set by either part (the same values).
static ALLOC: Installed<Alloc> = Installed::new();
/// Set if the merge replacement installed.
static MERGE: Installed<Merge> = Installed::new();
static GROWS: Counter = Counter::new();
static MERGES: Counter = Counter::new();
/// Merges left to BYOND.
static FALLBACKS: Counter = Counter::new();

/// Replaces the insert (Windows thiscall: map; key, flag byte).
///
/// # Safety
/// `map` is a live BYOND map.
#[cfg(windows)]
unsafe extern "thiscall" fn insert(map: *mut Map, key: u32, flag: u32) -> *mut u8 {
    // SAFETY: a live map (caller).
    unsafe { insert_into(map, key, flag as u8) }
}

/// Replaces the insert (Linux cdecl: map, key, flag byte).
///
/// # Safety
/// `map` is a live BYOND map.
#[cfg(target_os = "linux")]
unsafe extern "C" fn insert(map: *mut Map, key: u32, flag: u32) -> *mut u8 {
    // SAFETY: a live map (caller).
    unsafe { insert_into(map, key, flag as u8) }
}

/// The entry for `key`, inserted as `{key, 0, 0u16, flag}` if absent; null if the array could not grow.
///
/// # Safety
/// `map` is a live map.
unsafe fn insert_into(map: *mut Map, key: u32, flag: u8) -> *mut u8 {
    // SAFETY: a live map (caller): `count` entries at `entries`, room for one more after `reserve`.
    unsafe {
        let m = &mut *map;
        let count = m.count;
        // BYOND's search: lo/hi as u16, mid = (lo + hi) / 2. Merges insert in key order: try past the last entry first.
        let last_below = count > 0 && key_of(at(m, count as usize - 1)) < key;
        let (mut lo, mut hi) = if last_below { (count, count) } else { (0u16, count) };
        while lo < hi {
            let mid = ((lo as u32 + hi as u32) >> 1) as u16;
            let e = at(m, mid as usize);
            let k = key_of(e);
            if k < key {
                lo = mid + 1;
            } else if k > key {
                hi = mid;
            } else {
                return e;
            }
        }
        if !reserve(m, count as usize + 1) {
            return std::ptr::null_mut();
        }
        let e = at(m, lo as usize);
        if lo < count {
            std::ptr::copy(e, e.add(ENTRY), (count - lo) as usize * ENTRY);
        }
        m.count = count.wrapping_add(1);
        (e as *mut u32).write_unaligned(key);
        (e.add(4) as *mut u32).write_unaligned(0);
        (e.add(8) as *mut u16).write_unaligned(0);
        *e.add(10) = flag;
        e
    }
}

/// Makes room for `entries` entries, reallocating to 1.125x the need when the usable size is short. On failure
/// behaves as BYOND's insert (count = the null result's low word, then the out-of-memory handler) and returns false.
///
/// # Safety
/// `m` is a live map.
unsafe fn reserve(m: &mut Map, entries: usize) -> bool {
    let a = ALLOC.get();
    // SAFETY: a live map's array (caller), BYOND's allocator and handler (found at install).
    unsafe {
        let need = entries * ENTRY;
        if m.entries.is_null() || a.usable.get()(m.entries as *mut c_void) < need {
            GROWS.bump();
            let grown = a.realloc.get()(m.entries as *mut c_void, need + need / 8) as *mut u8;
            m.entries = grown;
            if grown.is_null() {
                m.count = 0;
                a.out_of_memory.get()();
                return false;
            }
        }
        true
    }
}

// Entry accessors. Safety: `m` is a live map with more than `i` entries, `e` a live entry.
unsafe fn at(m: &Map, i: usize) -> *mut u8 {
    // SAFETY: within the map's array (caller).
    unsafe { m.entries.add(i * ENTRY) }
}
unsafe fn key_of(e: *const u8) -> u32 {
    // SAFETY: a live entry (caller).
    unsafe { (e as *const u32).read_unaligned() }
}
unsafe fn values_of(e: *const u8) -> u32 {
    // SAFETY: a live entry (caller).
    unsafe { (e.add(4) as *const u32).read_unaligned() }
}
unsafe fn count_of(e: *const u8) -> u16 {
    // SAFETY: a live entry (caller).
    unsafe { (e.add(8) as *const u16).read_unaligned() }
}

/// Merges `src` into `dst` as BYOND's merge does. Returns false, having changed nothing, for cases left to BYOND:
/// no replacement installed, a map merged into itself, or a result too large for the u16 count.
/// Per source entry with values, in key order: a missing key is added as `{key, 0, 0, source flag}`. A destination
/// entry without values shares the source's array; else it gets a new array of its values then the source's, and
/// (flag 0) the proc of its last old value, if its parent link (+0x28) is 0xFFFF, is linked to the first appended.
///
/// # Safety
/// `dst` is a live BYOND map, `src` one or null, and no other thread uses them.
unsafe fn merge_into(dst: *mut Map, src: *const Map) -> bool {
    if !MERGE.is_set() {
        return false;
    }
    let ops = MERGE.get();
    // SAFETY: live maps (caller) and their value arrays; BYOND's functions (found at install).
    unsafe {
        if src.is_null() || (*src).count == 0 {
            return true;
        }
        if std::ptr::eq(dst, src) {
            FALLBACKS.bump();
            return false;
        }
        let (d, s) = (&mut *dst, &*src);
        let (dc, sc) = (d.count as usize, s.count as usize);
        if dc + sc > u16::MAX as usize {
            FALLBACKS.bump();
            return false;
        }
        // The merged entries go to a new array (at most dc + sc entries), in one pass; the old one is freed.
        let out = ops.malloc.get()((dc + sc) * ENTRY) as *mut u8;
        if out.is_null() {
            FALLBACKS.bump();
            return false;
        }
        MERGES.bump();
        let before = verify::before(d, s);
        let (mut i, mut o) = (0, 0);
        for j in 0..sc {
            // Source refcounts are scattered: prefetch a few entries ahead so the misses overlap.
            let ahead = values_of(at(s, (j + PREFETCH).min(sc - 1)));
            if ahead != 0 {
                prefetch(ahead as usize - 4);
            }
            let se = at(s, j);
            if count_of(se) == 0 {
                continue;
            }
            let sk = key_of(se);
            while i < dc && key_of(at(d, i)) < sk {
                std::ptr::copy_nonoverlapping(at(d, i), out.add(o * ENTRY), ENTRY);
                (i, o) = (i + 1, o + 1);
            }
            let de = out.add(o * ENTRY);
            o += 1;
            if i < dc && key_of(at(d, i)) == sk {
                std::ptr::copy_nonoverlapping(at(d, i), de, ENTRY);
                i += 1;
                if !take_values(ops, de, se) {
                    break; // out of memory: as BYOND, the merge stops here
                }
            } else {
                // BYOND adds {key, 0, 0, flag} and shares the source's array: a copy of the entry, one more reference.
                std::ptr::copy_nonoverlapping(se, de, ENTRY);
                add_ref(values_of(se));
            }
        }
        if i < dc {
            std::ptr::copy_nonoverlapping(at(d, i), out.add(o * ENTRY), (dc - i) * ENTRY);
            o += dc - i;
        }
        ops.free.get()(d.entries as *mut c_void);
        d.entries = out;
        d.count = o as u16;
        verify::check(before, d);
        true
    }
}

fn prefetch(addr: usize) {
    // SAFETY: a prefetch is a hint: it never faults.
    unsafe { std::arch::x86::_mm_prefetch::<{ std::arch::x86::_MM_HINT_T0 }>(addr as *const i8) };
}

/// `CopyShared`'s effect (checked at install): atomically one more reference in a non-null array's header dword.
///
/// # Safety
/// `values` is 0 or a live shared array.
unsafe fn add_ref(values: u32) {
    if values != 0 {
        // SAFETY: a live array (caller): its refcount dword, only changed atomically.
        unsafe { AtomicU32::from_ptr((values as usize - 4) as *mut u32) }.fetch_add(1, Relaxed);
    }
}

/// Gives entry `de` the values of `se` as BYOND's merge does. False if out of memory (BYOND's handler has run).
///
/// # Safety
/// `de` and `se` are live entries.
unsafe fn take_values(ops: &Merge, de: *mut u8, se: *const u8) -> bool {
    // SAFETY: live entries (caller) and their value arrays; BYOND's functions (found at install).
    unsafe {
        let (dn, sn) = (count_of(de), count_of(se));
        if dn == 0 {
            ops.free_shared.get()(values_of(de));
            (de.add(4) as *mut u32).write_unaligned(ops.copy_shared.get()(values_of(se)));
            (de.add(8) as *mut u16).write_unaligned(sn);
            return true;
        }
        let (dn, snn) = (dn as usize, sn as usize);
        let joined = ops.alloc_shared.get()((dn + snn) * 4);
        if joined == 0 {
            ALLOC.get().out_of_memory.get()();
            return false;
        }
        let out = joined as usize as *mut u32;
        std::ptr::copy_nonoverlapping(values_of(de) as usize as *const u32, out, dn);
        std::ptr::copy_nonoverlapping(values_of(se) as usize as *const u32, out.add(dn), snn);
        ops.free_shared.get()(values_of(de));
        (de.add(4) as *mut u32).write_unaligned(joined);
        if *de.add(10) == 0 {
            let p = ops.proc_at.get()(*out.add(dn - 1));
            if p != 0 && (p as *const u32).add(10).read_unaligned() == 0xFFFF {
                (p as *mut u32).add(10).write_unaligned(*out.add(dn));
            }
        }
        (de.add(8) as *mut u16).write_unaligned((dn as u16).wrapping_add(sn));
        true
    }
}

fn print_stats() {
    eprintln!("dmfast: sorted map insert grows={GROWS}");
    if MERGE.is_set() {
        eprintln!(
            "dmfast: sorted map merge merges={MERGES} fallbacks={FALLBACKS} verify={} checked={} mismatches={}",
            crate::base::driver::verify(),
            verify::CHECKED,
            verify::MISMATCHES
        );
    }
}
