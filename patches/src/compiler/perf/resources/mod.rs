//! `resources`: the resource name index. The compiler looks files up in a resource store by name, walking the
//! store's entry list with a strcmp per entry. A per-store index from name to first entry answers instead, built from
//! the list and checked against the entry's current name on every hit; a name it doesn't know (an entry added since)
//! runs the same first-match walk, which is recorded.
//!
//! An index is rebuilt after any call of the stores' remove function, the only way entries leave a store besides
//! clearing or destroying it. Assumed, not checked: a store is cleared or destroyed only when it's loaded, before any
//! lookup, or at teardown, and a name already listed is added again only through the add path, which removes it
//! first. `deps` guards the lookup and the remove.

use std::arch::naked_asm;
use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::Result;
use crate::base::code::{Addr, live_u32};
use crate::base::feature::{Feature, Hosts, install};
use crate::base::hash;
use crate::base::sync::Owned;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static RESOURCES: Feature = Feature::perf(
    Hosts::COMPILER,
    "resources",
    "resource index",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// A store's first entry; an entry's next one and its name (a C string or 0).
const HEAD: usize = 0x84;
const NEXT: usize = 0xC;
#[cfg(windows)]
const NAME: usize = 0x30;
#[cfg(target_os = "linux")]
const NAME: usize = 0x2C;

/// [`lookup`]'s answer for the stubs: run the original walk.
const FALLBACK: usize = 1;

/// Calls of the stores' remove function (any thread), bumped by [`remove_stub`]: an index built before one is stale.
static REMOVES: AtomicU32 = AtomicU32::new(0);
/// The stores' remove function: its moved entry, then the rest.
static REMOVE: Addr = Addr::new();

/// A store's index: each name's first entry, as the list was when [`REMOVES`] was `built_at`.
#[derive(Default)]
struct StoreIndex {
    built_at: Option<u32>,
    by_name: hash::Map<Box<[u8]>, u32>,
}

impl StoreIndex {
    /// Rebuild from `store`'s list if a remove ran since it was built: whether it did.
    ///
    /// # Safety
    /// `store` is a live store.
    unsafe fn refresh(&mut self, store: usize) -> bool {
        let removes = REMOVES.load(Relaxed);
        if self.built_at == Some(removes) {
            return false;
        }
        self.by_name.clear();
        // SAFETY: a live store's entries (caller).
        for e in unsafe { entries(store) } {
            if let Some(n) = unsafe { name_of(e) } {
                self.by_name.entry(n.into()).or_insert(e as u32);
            }
        }
        self.built_at = Some(removes);
        true
    }
}

/// Owned by the compiler's thread; the counts are plain adds on the hot path.
struct State {
    stores: hash::Map<u32, StoreIndex>,
    hits: u64,
    walks: u64,
    builds: u64,
}

static STATE: Owned<State> = Owned::new();

/// Set the state (the stubs' install, once everything is checked).
fn init() -> Result<()> {
    STATE.set(State { stores: hash::map(), hits: 0, walks: 0, builds: 0 })
}

/// `store`'s entries in list order.
///
/// # Safety
/// `store` is a live store, and stays so while the entries are read.
unsafe fn entries(store: usize) -> impl Iterator<Item = usize> {
    // SAFETY: a live store's list of live entries (caller).
    let link = |at: usize| Some(unsafe { live_u32(at) } as usize).filter(|&e| e != 0);
    std::iter::successors(link(store + HEAD), move |&e| link(e + NEXT))
}

/// # Safety
/// `entry` is a live entry of a store.
unsafe fn name_of<'a>(entry: usize) -> Option<&'a [u8]> {
    // SAFETY: a live entry (caller): its name pointer, 0 or a C string.
    unsafe {
        let p = live_u32(entry + NAME) as usize;
        (p != 0).then(|| CStr::from_ptr(p as *const c_char).to_bytes())
    }
}

/// The original walk: the first entry named `key`, or 0.
///
/// # Safety
/// `store` is a live store.
unsafe fn walk(store: usize, key: &[u8]) -> usize {
    // SAFETY: a live store's entries (caller).
    unsafe { entries(store).find(|&e| name_of(e) == Some(key)).unwrap_or(0) }
}

/// The first entry of `store` named `name`, 0 if there is none, or [`FALLBACK`]. Called by the walk stubs.
///
/// # Safety
/// `store` is a live store and `name` 0 or a C string.
unsafe extern "C" fn lookup(store: usize, name: *const c_char) -> usize {
    let Some(st) = STATE.get() else { return FALLBACK };
    if name.is_null() {
        return FALLBACK;
    }
    // SAFETY: a live store and name (caller); the store's entries are live.
    unsafe {
        let key = CStr::from_ptr(name).to_bytes();
        let idx = st.stores.entry(store as u32).or_default();
        if idx.refresh(store) {
            st.builds += 1;
        }
        let r = match idx.by_name.get(key) {
            Some(&e) if name_of(e as usize) == Some(key) => {
                st.hits += 1;
                e as usize
            }
            _ => {
                st.walks += 1;
                let e = walk(store, key);
                if e != 0 {
                    idx.by_name.insert(key.into(), e as u32);
                }
                e
            }
        };
        if !verify::agrees(r, || walk(store, key)) {
            return FALLBACK;
        }
        r
    }
}

/// Hooks the stores' remove function: count the call, then run it.
///
/// # Safety
/// Only jumped to from the remove function's entry.
#[unsafe(naked)]
unsafe extern "C" fn remove_stub() {
    naked_asm!(
        "lock inc dword ptr [{removes}]",
        "jmp dword ptr [{original}]",
        removes = sym REMOVES,
        original = sym REMOVE,
    )
}

fn print_stats() {
    // SAFETY: exit-time stats: the compiler no longer runs the hook.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: resource index hits={} walks={} builds={} removes={} verify={} mismatches={}",
            st.hits,
            st.walks,
            st.builds,
            REMOVES.load(Relaxed),
            crate::base::driver::verify(),
            verify::MISMATCHES
        );
    }
}
