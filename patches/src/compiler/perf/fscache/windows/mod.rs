//! `fscache`, Windows. Cache keys are exact paths; relative ones are prefixed with the current directory, re-read
//! after `_wchdir`/`_chdir`. byondcore imports no `FindNextFile*` (install checks), so a find handle only reaches
//! `FindClose`, and cached finds return fake handles from a reserved range. `SHFileOperationW` turns the cache off.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};

use rustc_hash::{FxHashMap, FxHashSet};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError};
use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
use windows_sys::Win32::Storage::FileSystem::{GetFullPathNameW, WIN32_FIND_DATAW};
use windows_sys::Win32::System::Environment::GetCurrentDirectoryW;
use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc};

use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::{Owned, lock};
use crate::base::windows::ucrt::ucrt_export;
use crate::base::{Result, bail};

mod listing;
mod modifiers;
mod verify;
use listing::Listings;
use modifiers::{FILE_SYSTEM, HOST, modifiers};

/// ucrtbase's `_errno`: where byondcore's `_wstat64i32` leaves errno and where byondcore reads it.
static ERRNO: Foreign<unsafe extern "C" fn() -> *mut i32> = Foreign::new();

unsafe fn ucrt_errno() -> *mut i32 {
    unsafe { ERRNO.get()() }
}

/// Set by SHFileOperationW: cache off for good.
static OFF: AtomicBool = AtomicBool::new(false);
/// Bumped by every modification; the owner thread clears its maps when it changes.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Normalized paths this process modified, plus their ancestors.
static VOLATILE: Mutex<Option<FxHashSet<Box<[u16]>>>> = Mutex::new(None);
static MODIFICATIONS: AtomicU32 = AtomicU32::new(0);
/// Bumped by `_wchdir`: the owner thread re-reads the current directory.
static CWD_GEN: AtomicU32 = AtomicU32::new(0);
static FAKE_BASE: AtomicUsize = AtomicUsize::new(0);
const FAKE_SLOTS: usize = 0x4000;

// The original readers, from their import slots.
static FIND_FIRST: Foreign<unsafe extern "system" fn(*const u16, *mut WIN32_FIND_DATAW) -> HANDLE> = Foreign::new();
static FIND_CLOSE: Foreign<unsafe extern "system" fn(HANDLE) -> i32> = Foreign::new();
static GET_ATTR: Foreign<unsafe extern "system" fn(*const u16) -> u32> = Foreign::new();
static GET_ATTR_EX: Foreign<unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32> = Foreign::new();
static STAT: Foreign<unsafe extern "C" fn(*const u16, *mut u8) -> i32> = Foreign::new();

#[derive(Clone)]
struct Find {
    ok: bool,
    data: WIN32_FIND_DATAW,
    err: u32,
}

/// A `_wstat64i32` result: return value, `struct _stat64i32`, errno.
type Stat = (i32, [u8; 48], i32);

#[derive(Default)]
struct Counters {
    hits: u64,
    calls: u64,
}

/// Hits and calls of each cached reader.
#[derive(Default)]
struct Readers {
    find: Counters,
    attr: Counters,
    attr_ex: Counters,
    stat: Counters,
}

type Cache<V> = FxHashMap<Box<[u16]>, V>;

#[derive(Default)]
struct State {
    generation: u32,
    cwd_gen: u32,
    /// Current directory; prefixed (NUL-separated) to relative keys.
    cwd: Vec<u16>,
    find: Cache<Find>,
    attr: Cache<(u32, u32)>,
    attr_ex: Cache<(i32, [u8; 36], u32)>,
    stat: Cache<Stat>,
    listings: Listings,
    next_fake: usize,
    readers: Readers,
}

static STATE: Owned<State> = Owned::new();

/// The cache, if enabled and the calling thread owns it; cleared if anything was modified since.
fn state() -> Option<&'static mut State> {
    if OFF.load(Relaxed) {
        return None;
    }
    let st = STATE.get()?;
    let generation = GENERATION.load(Relaxed);
    if st.generation != generation {
        st.generation = generation;
        st.find.clear();
        st.attr.clear();
        st.attr_ex.clear();
        st.stat.clear();
        st.listings.clear();
    }
    let cwd_gen = CWD_GEN.load(Relaxed);
    if st.cwd_gen != cwd_gen {
        st.cwd_gen = cwd_gen;
        let mut buf = vec![0u16; 1024];
        // SAFETY: a buffer of the length passed.
        let n = unsafe { GetCurrentDirectoryW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        if n == 0 || n >= buf.len() {
            return None; // unknown cwd: relative keys would be ambiguous
        }
        buf.truncate(n);
        st.cwd = buf;
    }
    Some(st)
}

unsafe fn wslice<'a>(p: *const u16) -> &'a [u16] {
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        std::slice::from_raw_parts(p, n)
    }
}

/// Full path, ASCII-lowercased, `/` -> `\`, no trailing `\`.
fn normalize(path: &[u16]) -> Box<[u16]> {
    let z: Vec<u16> = path.iter().copied().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: a NUL-terminated path and a buffer of the length passed.
    let n = unsafe { GetFullPathNameW(z.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    let full = if n == 0 || n >= buf.len() { path } else { &buf[..n] };
    let mut v: Vec<u16> = full
        .iter()
        .map(|&c| match c {
            0x2F => 0x5C,
            0x41..=0x5A => c + 0x20,
            _ => c,
        })
        .collect();
    while v.len() > 3 && v.last() == Some(&0x5C) {
        v.pop();
    }
    v.into()
}

fn parent(p: &[u16]) -> Option<&[u16]> {
    p.iter().rposition(|&c| c == 0x5C).map(|i| &p[..i])
}

/// Was `n` (normalized) modified by this process, or is it an ancestor of something that was?
fn is_volatile(n: &[u16]) -> bool {
    lock(&VOLATILE).as_ref().is_some_and(|set| set.contains(n))
}

/// May `key` be cached? No if it (or, for wildcards, its directory) was modified by this process.
fn cacheable(key: &[u16]) -> bool {
    let guard = lock(&VOLATILE);
    let Some(set) = guard.as_ref() else { return true };
    let n = normalize(key);
    if set.contains(&n) {
        return false;
    }
    let wild = key.iter().any(|&c| c == u16::from(b'*') || c == u16::from(b'?'));
    !(wild && parent(&n).is_some_and(|d| set.contains(d)))
}

/// Record a modification of `path` (wide, no NUL).
fn touched(path: &[u16]) {
    let n = normalize(path);
    let mut guard = lock(&VOLATILE);
    let set = guard.get_or_insert_with(FxHashSet::default);
    let mut p: &[u16] = &n;
    loop {
        set.insert(p.into());
        match parent(p) {
            Some(d) if !d.is_empty() => p = d,
            _ => break,
        }
    }
    drop(guard);
    MODIFICATIONS.fetch_add(1, Relaxed);
    GENERATION.fetch_add(1, Relaxed);
}

unsafe fn touched_w(p: *const u16) {
    if !p.is_null() {
        touched(unsafe { wslice(p) });
    }
}

/// Narrow (ANSI code page) path variant.
unsafe fn touched_a(p: *const u8) {
    if p.is_null() {
        return;
    }
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, p, -1, std::ptr::null_mut(), 0);
        if n <= 0 {
            OFF.store(true, Relaxed); // cannot tell which path: stop caching
            return;
        }
        let mut w = vec![0u16; n as usize];
        MultiByteToWideChar(CP_ACP, 0, p, -1, w.as_mut_ptr(), n);
        w.pop();
        touched(&w);
    }
}

/// Look `name` up in `map`, or compute it with `fetch` and cache it when allowed. Relative names are keyed with `cwd`.
unsafe fn cached<V: Clone>(
    name: *const u16,
    cwd: &[u16],
    map: &mut Cache<V>,
    c: &mut Counters,
    fetch: impl FnOnce() -> V,
) -> V {
    let path = unsafe { wslice(name) };
    c.calls += 1;
    // Any drive letter or UNC path: independent of the current directory.
    let absolute = matches!(path, [_, 0x3A, 0x5C | 0x2F, ..] | [0x5C, 0x5C, ..] | [0x2F, 0x2F, ..]);
    // `D:foo` and `\foo` resolve against per-drive current directories we do not track: never cache.
    if !absolute && matches!(path, [_, 0x3A, ..] | [0x5C | 0x2F, ..]) {
        return fetch();
    }
    let rel_key;
    let key: &[u16] = if absolute {
        path
    } else {
        rel_key = [cwd, &[0], path].concat();
        &rel_key
    };
    if let Some(v) = map.get(key) {
        c.hits += 1;
        return v.clone();
    }
    let v = fetch();
    // A modification during `fetch` (other thread) bumps the generation: the next lookup clears the map.
    if cacheable(path) {
        map.insert(key.into(), v.clone());
    }
    v
}

// ---- cached readers -------------------------------------------------------------------------------

unsafe extern "system" fn find_first_file_w(name: *const u16, data: *mut WIN32_FIND_DATAW) -> HANDLE {
    unsafe {
        let Some(st) = state() else { return FIND_FIRST.get()(name, data) };
        let listings = &mut st.listings;
        let e = cached(name, &st.cwd, &mut st.find, &mut st.readers.find, || {
            let real = || {
                let mut d: WIN32_FIND_DATAW = std::mem::zeroed();
                let h = FIND_FIRST.get()(name, &mut d);
                let err = GetLastError();
                let ok = h != INVALID_HANDLE_VALUE;
                if ok {
                    FIND_CLOSE.get()(h);
                }
                Find { ok, data: d, err }
            };
            let Some((entry, _)) = listings.lookup(wslice(name)) else { return real() };
            let ans = match entry {
                Some(d) => Find { ok: true, data: d, err: GetLastError() },
                None => Find { ok: false, data: std::mem::zeroed(), err: 2 }, // ERROR_FILE_NOT_FOUND
            };
            listings.counters.find += 1;
            verify::find(ans, real, &mut listings.counters)
        });
        SetLastError(e.err);
        if !e.ok {
            return INVALID_HANDLE_VALUE;
        }
        *data = e.data;
        st.next_fake = (st.next_fake + 1) % FAKE_SLOTS;
        (FAKE_BASE.load(Relaxed) + 4 * st.next_fake) as HANDLE
    }
}

unsafe extern "system" fn find_close(h: HANDLE) -> i32 {
    let base = FAKE_BASE.load(Relaxed);
    if (base..base + 4 * FAKE_SLOTS).contains(&(h as usize)) {
        return 1;
    }
    unsafe { FIND_CLOSE.get()(h) }
}

unsafe extern "system" fn get_file_attributes_w(name: *const u16) -> u32 {
    unsafe {
        let Some(st) = state() else { return GET_ATTR.get()(name) };
        let (r, err) =
            cached(name, &st.cwd, &mut st.attr, &mut st.readers.attr, || (GET_ATTR.get()(name), GetLastError()));
        SetLastError(err);
        r
    }
}

unsafe extern "system" fn get_file_attributes_ex_w(name: *const u16, level: i32, info: *mut c_void) -> i32 {
    unsafe {
        let st = if level == 0 { state() } else { None }; // GetFileExInfoStandard only
        let Some(st) = st else { return GET_ATTR_EX.get()(name, level, info) };
        let (r, data, err) = cached(name, &st.cwd, &mut st.attr_ex, &mut st.readers.attr_ex, || {
            let mut d = [0u8; 36];
            let r = GET_ATTR_EX.get()(name, level, d.as_mut_ptr().cast());
            (r, d, GetLastError())
        });
        if r != 0 {
            std::ptr::copy_nonoverlapping(data.as_ptr(), info.cast::<u8>(), 36);
        }
        SetLastError(err);
        r
    }
}

unsafe extern "C" fn wstat64i32(name: *const u16, buf: *mut u8) -> i32 {
    unsafe {
        let Some(st) = state() else { return STAT.get()(name, buf) };
        let listings = &mut st.listings;
        let (r, data, errno) = cached(name, &st.cwd, &mut st.stat, &mut st.readers.stat, || {
            let real = || {
                let mut d = [0u8; 48];
                let r = STAT.get()(name, d.as_mut_ptr());
                (r, d, *ucrt_errno())
            };
            let path = wslice(name);
            let Some((entry, drive)) = listings.lookup(path) else { return real() };
            let ans = match entry {
                None => Some((-1, [0u8; 48], 2)), // ENOENT
                Some(e) => listing::stat_of(&e, drive, path).map(|d| (0, d, 0)),
            };
            let Some(ans) = ans else { return real() };
            listings.counters.stat += 1;
            verify::stat(ans, real, &mut listings.counters)
        });
        if r == 0 {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, 48);
        } else {
            *ucrt_errno() = errno;
        }
        r
    }
}

/// The image of a watched module: byondcore's, the program's, or another loaded DLL (`others`).
fn module<'a>(name: &str, cx: &'a Ctx, others: &'a [(&str, Image)]) -> Result<&'a Image> {
    match name {
        "byondcore.dll" => Ok(cx.core()),
        HOST => cx.exe(),
        _ => others.iter().find(|(n, _)| *n == name).map(|(_, m)| m).ok_or_else(|| format!("{name} not loaded").into()),
    }
}

/// Reserves `len` bytes of address space, never committed: addresses no one else gets.
fn reserve(len: usize) -> Result<usize> {
    // SAFETY: a fresh reservation; nothing is read or written through it.
    let p = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_RESERVE, PAGE_NOACCESS) } as usize;
    if p == 0 { Err("VirtualAlloc failed".into()) } else { Ok(p) }
}

/// The dword in an import slot as it is now.
fn slot_value(image: &Image, slot: usize) -> Result<usize> {
    Ok(image.live_u32(slot)? as usize)
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // Independent parts (FindFirstFileW's fake handles only our FindClose takes). Readers are cached only if every
    // modifier is watched and the watched modules import no other file system modifier or find-handle call.
    type Reader = (&'static str, fn(usize), usize);
    // Each part, and whether it needs stat's extras: stat answers carry ucrtbase's errno and times.
    let groups: [(&[Reader], bool); 4] = [
        (
            &[
                ("FindFirstFileW", |a| FIND_FIRST.set(a), find_first_file_w as *const () as usize),
                ("FindClose", |a| FIND_CLOSE.set(a), find_close as *const () as usize),
            ],
            false,
        ),
        (&[("GetFileAttributesW", |a| GET_ATTR.set(a), get_file_attributes_w as *const () as usize)], false),
        (&[("GetFileAttributesExW", |a| GET_ATTR_EX.set(a), get_file_attributes_ex_w as *const () as usize)], false),
        (&[("_wstat64i32", |a| STAT.set(a), wstat64i32 as *const () as usize)], true),
    ];
    let mut parts = cx.parts();
    let mut readers = Vec::new();
    for (group, stat) in groups {
        parts.part(|| {
            if stat {
                core.ucrt_slot("_wstat64i32")?;
                ERRNO.set(ucrt_export("_errno")?);
                listing::resolve()?;
            }
            let slots = group.iter().map(|r| core.iat_slot(r.0).map(|s| (r, s))).collect::<Result<Vec<_>, _>>()?;
            readers.push(slots);
            Ok(group.iter().map(|r| r.0).collect::<Vec<_>>().join(", "))
        });
    }
    if !parts.any() {
        return parts.finish();
    }
    let modifiers = modifiers();
    let mut others: Vec<(&str, Image)> = Vec::new();
    let mut modifier_slots = Vec::new();
    for m in &modifiers {
        if !matches!(m.module, "byondcore.dll" | HOST) && !others.iter().any(|(n, _)| *n == m.module) {
            others.push((m.module, cx.module(m.module)?));
        }
        modifier_slots.push((m, module(m.module, cx, &others)?.iat_slot(m.import)?));
    }
    let mut modules: Vec<&str> = modifiers.iter().map(|m| m.module).collect();
    modules.sort_unstable();
    modules.dedup();
    for module_name in modules {
        let image = module(module_name, cx, &others)?;
        let watched = |n: &str| modifiers.iter().any(|m| m.module == module_name && m.import == n);
        if let Some(name) = FILE_SYSTEM.iter().find(|&&n| image.iat_slot(n).is_ok() && !watched(n)) {
            bail!("{module_name} imports {name}, which the cache does not watch");
        }
    }
    FAKE_BASE.store(reserve(4 * FAKE_SLOTS)?, Relaxed);
    STATE.set(State { cwd_gen: u32::MAX, ..Default::default() })?;
    for (m, slot) in modifier_slots {
        let image = module(m.module, cx, &others)?;
        (m.set_original)(slot_value(image, slot)?);
        image.write_data(slot, &m.hook.to_le_bytes())?;
    }
    // The parts' slots are written as the feature's own: all of them or none.
    for part in readers {
        for (&(_, set_original, ours), slot) in part {
            set_original(slot_value(core, slot)?);
            core.write_data(slot, &ours.to_le_bytes())?;
        }
    }
    parts.finish()
}

pub fn print_stats() {
    // SAFETY: exit-time stats; the owner no longer runs hooks.
    if let Some(st) = unsafe { STATE.shared() } {
        let r = &st.readers;
        let hits = |c: &Counters| format!("{}/{}", c.hits, c.calls);
        let l = &st.listings.counters;
        eprintln!(
            "dmfast: file cache hits/calls find {}, attr {}, attr_ex {}, stat {}; from listings find {} stat {} ({} dirs listed, verify={} mismatches={}); modifications={} off={}",
            hits(&r.find),
            hits(&r.attr),
            hits(&r.attr_ex),
            hits(&r.stat),
            l.find,
            l.stat,
            l.lists,
            crate::base::driver::verify(),
            l.mismatches,
            MODIFICATIONS.load(Relaxed),
            OFF.load(Relaxed)
        );
    }
}
