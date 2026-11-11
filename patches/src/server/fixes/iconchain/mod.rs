//! `iconchain`: an icon rebuilt from its own runtime icon over and over doesn't make clients load a chain of icons.
//!
//! A runtime `/icon` stored as a resource keeps its operation list, which loads the icons it was made from by resource
//! id. Rebuilding an icon from its own current icon makes each resource load the one before; a client fetches the
//! chain one link at a time and keeps every link drawn, so a few hundred links take minutes and gigabytes.
//!
//! The fix tracks each stored list's depth (one more than the deepest stored list it loads). A list that would load
//! one [`MAX_DEPTH`] deep is drawn and stored as pixels instead, as BYOND does for a list over 256 bytes. BYOND's
//! server draws a source again for every load of it (2^depth draws for a link that loads the one before twice), so
//! while the fix draws, the server's icon context keeps a copy of each icon it loads and answers repeat loads with it.
//!
//! Safe: BYOND's own render and flatten, on the path it takes for a long list, so the icon looks the same. Extended:
//! a game reading the resource (`file2text()`, `length()`) sees pixels every [`MAX_DEPTH`]th link.

use std::iter;
use std::sync::Mutex;

use crate::base::code::{self, Foreign, live_u32};
use crate::base::driver::Counter;
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::hash::{self, Map};
use crate::base::sync::{Installed, lock};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static ICONCHAIN: Feature = Feature::fix(
    Hosts::SERVER,
    "iconchain",
    "a runtime icon built on a chain of 12 runtime icons is stored as pixels",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// The longest chain of stored operation lists a client may have to load.
const MAX_DEPTH: u8 = 12;

/// The longest operation list BYOND stores as a list; it draws longer ones.
const LIST_MAX: usize = 0x100;

#[cfg(windows)]
mod abi {
    pub type Render = unsafe extern "thiscall" fn(usize);
    pub type Flatten = unsafe extern "fastcall" fn(usize);
    pub type Copy = unsafe extern "thiscall" fn(usize, usize) -> usize;
    pub type Destroy = unsafe extern "thiscall" fn(usize);
}
#[cfg(target_os = "linux")]
mod abi {
    pub type Render = unsafe extern "C" fn(usize);
    pub type Flatten = unsafe extern "C" fn(usize);
    pub type Copy = unsafe extern "C" fn(usize, usize);
    pub type Destroy = unsafe extern "C" fn(usize);
}

/// BYOND's render (the icon record) and flatten (its list), `DMIcon`'s copy constructor and destructor, and its walk
/// of an operation list.
struct Found {
    render: Foreign<abi::Render>,
    flatten: Foreign<abi::Flatten>,
    copy: Foreign<abi::Copy>,
    destroy: Foreign<abi::Destroy>,
    walk: Foreign<Walk>,
}

/// The next resource an operation list loads from the cursor (moved past it), or `u32::MAX` at its end. cdecl.
type Walk = unsafe extern "C" fn(*mut *const u8) -> u32;

static FOUND: Installed<Found> = Installed::new();

struct State {
    /// Each stored operation list's depth, by resource id.
    depths: Map<u32, u8>,
    /// The icon record being stored and the depth its list will have.
    pending: Option<(usize, u8)>,
    /// While the fix draws: a copy of each icon loaded so far, by resource id.
    loaded: Option<Map<u32, Box<Icon>>>,
}

static STATE: Mutex<State> = Mutex::new(State { depths: hash::map(), pending: None, loaded: None });

static FLATTENED: Counter = Counter::new();

fn print_stats() {
    eprintln!("dmfast: icon chains flattened={FLATTENED} lists={}", lock(&STATE).depths.len());
}

/// A copy of a `DMIcon` (1Ch bytes), destroyed when dropped.
#[repr(C, align(4))]
struct Icon([u8; 0x20]);

impl Icon {
    /// # Safety
    /// `icon` is a live `DMIcon`.
    unsafe fn copy_of(icon: usize) -> Box<Icon> {
        let copy = Box::new(Icon([0; 0x20]));
        // SAFETY: room for a DMIcon, constructed as a copy of BYOND's (caller).
        unsafe { FOUND.get().copy.get()(copy.addr(), icon) };
        copy
    }

    fn addr(&self) -> usize {
        self.0.as_ptr() as usize
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: constructed by `copy_of`, destroyed once.
        unsafe { FOUND.get().destroy.get()(self.addr()) }
    }
}

/// The resource ids a NUL-terminated operation list loads, through BYOND's own walk.
///
/// # Safety
/// `list` is an icon record's operation list, live while the iterator is.
unsafe fn loads(list: *const u8) -> impl Iterator<Item = u32> {
    // SAFETY: set at install.
    let walk = unsafe { FOUND.get().walk.get() };
    let mut cursor = list;
    // SAFETY: BYOND's walk over its own list (caller).
    iter::from_fn(move || match unsafe { walk(&mut cursor) } {
        u32::MAX => None,
        id => Some(id),
    })
}

/// Before an icon record is stored as a resource: note the depth its list will have, or draw the icon first if that
/// would be over [`MAX_DEPTH`]. Record: +8 the list, +0Ch its length, +14h the 16-bit drawn offset.
extern "C" fn prepare(record: usize) {
    let mut state = lock(&STATE);
    state.pending = None;
    // SAFETY: the conversion's icon record.
    let len = unsafe { live_u32(record + 0x0C) } as usize;
    // BYOND draws these itself.
    if len == 0 || len > LIST_MAX {
        return;
    }
    // SAFETY: the record's list is `len` bytes.
    let list = unsafe { std::slice::from_raw_parts(live_u32(record + 8) as usize as *const u8, len) };
    // Already drawn (`?`), or one load, stored as that resource.
    if list[0] == b'?' || (len == 6 && list[0] == 0x01) {
        return;
    }
    // SAFETY: the record's list (above).
    let deepest = unsafe { loads(list.as_ptr()) }.filter_map(|id| state.depths.get(&id).copied()).max().unwrap_or(0);
    if deepest < MAX_DEPTH {
        state.pending = Some((record, deepest + 1));
        return;
    }
    state.loaded = Some(hash::map());
    drop(state);
    FLATTENED.bump();
    let found = FOUND.get();
    // SAFETY: BYOND's render and flatten, called as the conversion calls them for a list it stores as pixels.
    unsafe {
        found.render.get()(record);
        found.flatten.get()(record + 8);
        code::write::<u16>(record + 0x14, live_u32(record + 0x0C) as u16);
    }
    // Destroys the copies, outside the lock.
    let loaded = lock(&STATE).loaded.take();
    drop(loaded);
}

/// The conversion stored the icon record as resource `id`: keep its depth if it was stored as an operation list.
fn stored(record: usize, id: u32) {
    let mut state = lock(&STATE);
    if let Some((r, depth)) = state.pending.take()
        && r == record
    {
        state.depths.insert(id, depth);
    }
}

/// The server icon context's look-up of a loaded icon, while the fix draws: the copy of resource `id`, if loaded.
/// BYOND keeps its own answer's icon for the caller, which copies it or uses it in the draw it is in.
fn loaded(id: u32) -> Option<usize> {
    lock(&STATE).loaded.as_ref()?.get(&id).map(|icon| icon.addr())
}

/// The server icon context's save of an icon it loaded (stock: nothing), while the fix draws: keep a copy.
///
/// # Safety
/// `icon` is the `DMIcon` BYOND loaded resource `id` into.
unsafe fn save(id: u32, icon: usize) {
    let mut state = lock(&STATE);
    if let Some(loaded) = state.loaded.as_mut() {
        // SAFETY: BYOND's loaded icon (caller).
        loaded.entry(id).or_insert_with(|| unsafe { Icon::copy_of(icon) });
    }
}
