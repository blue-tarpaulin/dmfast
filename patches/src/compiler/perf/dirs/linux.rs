//! `dirs`, Linux. The main listing gets the cache and the skipped change of directory; the prefix listing (path
//! completion) only skips opening entries that can't be directories.

use std::ffi::{CStr, OsStr, c_char, c_int};
use std::mem::offset_of;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use libc::{__errno_location, DIR, DT_DIR, DT_LNK, DT_UNKNOWN, ENOTDIR, dirent};
use rustc_hash::FxHashMap;

use crate::base::code::Foreign;
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::linux::symbol;
use crate::base::sync::{Owned, lock};
use crate::base::{Result, bail};

// libbyond reads `d_type` at +0Ah and `d_name` at +0Bh of glibc's 32-bit `struct dirent`.
const _: () = assert!(offset_of!(dirent, d_type) == 10 && offset_of!(dirent, d_name) == 11);

static OPENDIR: Foreign<unsafe extern "C" fn(*const c_char) -> *mut DIR> = Foreign::new();
static READDIR: Foreign<unsafe extern "C" fn(*mut DIR) -> *mut dirent> = Foreign::new();
static CLOSEDIR: Foreign<unsafe extern "C" fn(*mut DIR) -> c_int> = Foreign::new();
/// libbyond's `ChangeDir(path)`: chdir, then refresh its cached cwd with `getcwd`; 1 on success.
static CHANGE_DIR: Foreign<unsafe extern "C" fn(*const c_char) -> u8> = Foreign::new();

fn opendir(name: *const c_char) -> *mut DIR {
    // SAFETY: libc's opendir; `name` is a NUL-terminated path.
    unsafe { OPENDIR.get()(name) }
}

fn readdir(d: *mut DIR) -> *mut dirent {
    // SAFETY: libc's readdir on a stream libc opened.
    unsafe { READDIR.get()(d) }
}

fn closedir(d: *mut DIR) -> c_int {
    // SAFETY: libc's closedir on a stream libc opened.
    unsafe { CLOSEDIR.get()(d) }
}

fn change_dir(path: *const c_char) -> bool {
    // SAFETY: libbyond's ChangeDir (checked at install); `path` is the listing's directory string.
    unsafe { CHANGE_DIR.get()(path) != 0 }
}

fn not_dir() -> *mut DIR {
    // SAFETY: errno of the calling thread.
    unsafe { *__errno_location() = ENOTDIR };
    std::ptr::null_mut()
}

fn cannot_be_dir(d_type: u8) -> bool {
    !matches!(d_type, DT_DIR | DT_LNK | DT_UNKNOWN)
}

/// A cached `readdir` result. `dirent` comes first, so the `d_name` libbyond passes back leads to the entry.
#[repr(C)]
pub(super) struct Entry {
    pub(super) dirent: dirent,
    /// Whether it is a directory, once checked.
    is_dir: OnceLock<bool>,
}

type Listing = Arc<[Entry]>;

/// A cached listing is valid while its directory's mtime and ctime (seconds, nanoseconds) are unchanged.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Stamp {
    mtime: (i64, i64),
    ctime: (i64, i64),
}

/// A directory's device and inode, stamp, and whether it is a directory.
#[derive(Clone, Copy)]
pub(super) struct Stat {
    key: (u64, u64),
    pub(super) stamp: Stamp,
    is_dir: bool,
}

pub(super) fn stat(path: *const c_char) -> Option<Stat> {
    // SAFETY: a NUL-terminated path from libbyond.
    let path = OsStr::from_bytes(unsafe { CStr::from_ptr(path) }.to_bytes());
    let m = std::fs::metadata(path).ok()?;
    Some(Stat {
        key: (m.dev(), m.ino()),
        stamp: Stamp { mtime: (m.mtime(), m.mtime_nsec()), ctime: (m.ctime(), m.ctime_nsec()) },
        is_dir: m.is_dir(),
    })
}

/// Changed within the last two seconds, which coarse timestamps may not show.
fn too_recent(s: &Stat) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    now - s.stamp.mtime.0.max(s.stamp.ctime.0) < 2
}

pub(super) fn read_listing(path: *const c_char) -> Option<Listing> {
    let d = opendir(path);
    if d.is_null() {
        return None;
    }
    let entries = std::iter::from_fn(|| {
        let e = readdir(d);
        if e.is_null() {
            return None;
        }
        // The header and name only: an entry can be shorter than `dirent` (glibc before 2.37, the `inodes` fix).
        // SAFETY: `e` is readdir's entry, with a NUL-terminated name.
        let mut copy: dirent = unsafe { std::mem::zeroed() };
        unsafe {
            let len = CStr::from_ptr((&raw const (*e).d_name).cast()).count_bytes().min(255);
            std::ptr::copy_nonoverlapping(e.cast::<u8>(), (&raw mut copy).cast(), offset_of!(dirent, d_name) + len);
        }
        Some(Entry { dirent: copy, is_dir: OnceLock::new() })
    })
    .collect();
    closedir(d);
    Some(entries)
}

/// Each cached directory's stamp and listing, by device and inode.
type Dirs = FxHashMap<(u64, u64), (Stamp, Listing)>;

static CACHE: Mutex<Option<Dirs>> = Mutex::new(None);
static HITS: AtomicU32 = AtomicU32::new(0);
static WITHOUT_CHDIR: AtomicU32 = AtomicU32::new(0);
static READS: AtomicU32 = AtomicU32::new(0);
static OPENED: AtomicU32 = AtomicU32::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);

/// The directory of a listing whose `ChangeDir` was skipped: a NUL-terminated string that outlives the listing.
static PENDING: Owned<Option<*const c_char>> = Owned::new();

/// Do the skipped `ChangeDir`, if any. False if it failed.
pub(super) fn change_now() -> bool {
    PENDING.get().and_then(Option::take).is_none_or(change_dir)
}

/// Replaces the listing's `ChangeDir(dir)`. A cache hit doesn't need the directory, so the change waits until
/// something reads the real directory.
unsafe extern "C" fn enter(dir: *const c_char) -> u8 {
    if let Some(p) = PENDING.get()
        && !dir.is_null()
        && p.is_none()
    {
        *p = Some(dir);
        return 1;
    }
    // Another thread, no directory, or one already pending: change now, after any pending one.
    change_now();
    u8::from(change_dir(dir))
}

/// Replaces the listing's `ChangeDir(saved cwd)`: nothing to undo if the change never happened.
unsafe extern "C" fn leave(saved: *const c_char) -> u8 {
    if PENDING.get().and_then(Option::take).is_some() {
        return 1;
    }
    u8::from(change_dir(saved))
}

/// An open cached listing, handed to libbyond as its `DIR*`.
struct Cursor {
    listing: Listing,
    pos: usize,
}

/// Replaces the main listing's `opendir(".")`.
unsafe extern "C" fn list_opendir(path: *const c_char) -> *mut DIR {
    let mut guard = lock(&CACHE);
    let dirs = guard.as_mut().expect("dirs cache not initialised");
    // With the change of directory pending, "." is that directory.
    let pending = PENDING.get().and_then(|p| *p);
    let st = stat(pending.unwrap_or(path));
    if pending.is_some() && !st.is_some_and(|s| s.is_dir) {
        // ChangeDir would have failed, so there is no listing.
        return std::ptr::null_mut();
    }
    let cached = st.and_then(|s| dirs.get(&s.key).filter(|(stamp, _)| *stamp == s.stamp).map(|(_, l)| l.clone()));
    let listing = match cached {
        Some(l) => {
            HITS.fetch_add(1, Relaxed);
            if pending.is_some() {
                WITHOUT_CHDIR.fetch_add(1, Relaxed);
            }
            super::verify::hit(path, &l, st);
            l
        }
        None => {
            READS.fetch_add(1, Relaxed);
            if !change_now() {
                return std::ptr::null_mut();
            }
            let Some(l) = read_listing(path) else { return std::ptr::null_mut() };
            if let Some(s) = st.filter(|s| !too_recent(s)) {
                dirs.insert(s.key, (s.stamp, l.clone()));
            }
            l
        }
    };
    Box::into_raw(Box::new(Cursor { listing, pos: 0 })).cast()
}

unsafe extern "C" fn list_readdir(d: *mut DIR) -> *mut dirent {
    // SAFETY: `d` is a cursor from `list_opendir`.
    let c = unsafe { &mut *d.cast::<Cursor>() };
    let Some(e) = c.listing.get(c.pos) else { return std::ptr::null_mut() };
    c.pos += 1;
    (&raw const e.dirent).cast_mut()
}

unsafe extern "C" fn list_closedir(d: *mut DIR) -> c_int {
    // SAFETY: `d` is a cursor from `list_opendir`.
    drop(unsafe { Box::from_raw(d.cast::<Cursor>()) });
    0
}

/// Replaces the main listing's `opendir(entry->d_name)`; the entry is one of ours. A directory returns `name` as
/// its stream: libbyond only checks it isn't null and passes it to [`list_closedir_entry`].
unsafe extern "C" fn list_opendir_entry(name: *const c_char) -> *mut DIR {
    // SAFETY: `name` is the `d_name` of an `Entry` from `list_readdir`.
    let e = unsafe { &*name.sub(offset_of!(dirent, d_name)).cast::<Entry>() };
    let is_dir = match e.is_dir.get() {
        Some(&is_dir) => is_dir,
        None if cannot_be_dir(e.dirent.d_type) => {
            SKIPPED.fetch_add(1, Relaxed);
            *e.is_dir.get_or_init(|| false)
        }
        None => {
            OPENED.fetch_add(1, Relaxed);
            // The real opendir resolves `name` in the real directory.
            if !change_now() {
                return not_dir();
            }
            let d = opendir(name);
            if !d.is_null() {
                closedir(d);
            }
            *e.is_dir.get_or_init(|| !d.is_null())
        }
    };
    if is_dir { name.cast_mut().cast() } else { not_dir() }
}

unsafe extern "C" fn list_closedir_entry(_d: *mut DIR) -> c_int {
    0
}

/// Replaces the prefix listing's `opendir(entry->d_name)`; the entry is readdir's.
unsafe extern "C" fn complete_opendir_entry(name: *const c_char) -> *mut DIR {
    // SAFETY: `name` is a `d_name`, so the byte before it is `d_type`.
    if cannot_be_dir(unsafe { *name.sub(1) } as u8) {
        SKIPPED.fetch_add(1, Relaxed);
        return not_dir();
    }
    OPENED.fetch_add(1, Relaxed);
    opendir(name)
}

/// A call to replace: where, the function it must call, and the replacement.
type Site = (usize, usize, usize);

fn check(core: &Image, sites: &[Site]) -> Result<()> {
    for &(call, expect, _) in sites {
        if core.rel32(call + 1)? != expect {
            bail!("call at {call:#x} is not the expected function");
        }
    }
    Ok(())
}

/// The main listing's calls, including its `ChangeDir` into the directory and back.
fn main_listing(core: &Image) -> Result<Vec<Site>> {
    // push "." ; call opendir ; mov [ebp-54h],eax ; ... loop: call readdir ; lea ebx,[esi+0Bh] (d_name) ;
    // strncmp ; skip "." and ".." ; ... ; call opendir(ebx) ; ... ; push '/' ; ... ; call closedir(entry)
    let list = core.find_unique(
        "83 EC 0C 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 89 45 AC 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? \
         C7 45 A0 00 00 00 00 8D 74 26 00 83 EC 0C FF 75 AC E8 ?? ?? ?? ?? 89 C6 83 C4 10 85 C0 \
         0F 84 ?? ?? ?? ?? 83 EC 04 8D 5E 0B FF 75 A4 53 FF 75 A8 E8 ?? ?? ?? ?? 83 C4 10 85 C0 \
         75 CF 80 7E 0B 2E 75 0D 80 7E 0C 00 74 C3 66 83 7E 0C 2E 74 BC 83 EC 08 8D 45 C0 53 50 \
         E8 ?? ?? ?? ?? 89 1C 24 E8 ?? ?? ?? ?? 89 C3 83 C4 10 85 C0 74 19 83 EC 08 8D 45 C0 \
         6A 2F 50 E8 ?? ?? ?? ?? 89 1C 24 E8",
        "directory listing",
    )?;
    core.expect(core.ptr_at(list + 4)?, b".\0", "directory listing does not open \".\"")?;
    // Loop end: count < max -> next entry ; push [ebp-54h] ; call closedir
    let list_end = core.find_unique(
        "8B 4D A0 3B 4D 10 8D 41 01 89 45 A0 0F 8C ?? ?? ?? ?? 83 EC 0C FF 75 AC E8 ?? ?? ?? ?? \
         83 C4 10 8B 45 C4 85 C0 74 0C 83 EC 0C 50 E8",
        "directory listing end",
    )?;
    // ChangeDir(dir) right before the listing: call ; add esp,10h ; test al,al ; je fail ; push "." ; call opendir
    let enter_at = core.find_unique(
        "E8 ?? ?? ?? ?? 83 C4 10 84 C0 0F 84 ?? ?? ?? ?? 83 EC 0C 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 89 45 AC",
        "directory listing chdir",
    )?;
    if enter_at + 0x10 != list {
        bail!("directory listing chdir is not at the listing");
    }
    let change_dir = core.rel32(enter_at + 1)?;
    if core.bytes(change_dir, 14)?
        != [0x55, 0x89, 0xE5, 0x56, 0x53, 0x83, 0xEC, 0x10, 0x8B, 0x45, 0x08, 0x85, 0xC0, 0x74]
    {
        bail!("directory listing chdir is not ChangeDir");
    }
    let (opendir, readdir, closedir) = (OPENDIR.addr(), READDIR.addr(), CLOSEDIR.addr());
    let sites = vec![
        (enter_at, change_dir, enter as *const () as usize),
        (list + 8, opendir, list_opendir as *const () as usize),
        (list + 0x2C, readdir, list_readdir as *const () as usize),
        (list + 0x7A, opendir, list_opendir_entry as *const () as usize),
        (list + 0x99, closedir, list_closedir_entry as *const () as usize),
        (list_end + 0x18, closedir, list_closedir as *const () as usize),
        // After it: add esp,10h ; mov eax,[ebp-3Ch] (saved cwd) ; test eax,eax ; je ; push eax ; call ChangeDir
        (list_end + 0x2B, change_dir, leave as *const () as usize),
    ];
    check(core, &sites)?;
    CHANGE_DIR.set(change_dir);
    Ok(sites)
}

/// The prefix listing's `opendir(entry->d_name)`.
fn prefix_listing(core: &Image) -> Result<Vec<Site>> {
    // lea esi,[eax+0Bh] (d_name) ; strncmp ; ... ; strdup(esi) ; call opendir(esi)
    let complete = core.find_unique(
        "8D 70 0B FF 75 CC 56 FF 75 D0 E8 ?? ?? ?? ?? 83 C4 10 85 C0 75 D0 8B 45 D4 80 78 0B 2E \
         0F 84 ?? ?? ?? ?? 8B 45 0C 0F B7 CB 89 4D D4 8B 00 F6 C3 07 0F 84 ?? ?? ?? ?? 83 EC 0C \
         8B 55 D4 83 C3 01 56 8D 04 90 89 45 D4 E8 ?? ?? ?? ?? 8B 4D D4 89 01 89 34 24 E8",
        "prefix directory listing",
    )?;
    let sites = vec![(complete + 0x54, OPENDIR.addr(), complete_opendir_entry as *const () as usize)];
    check(core, &sites)?;
    Ok(sites)
}

/// Each listing is checked before any of its code is written; one that doesn't match is left out.
pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // libc's, as the program binds them.
    OPENDIR.set(symbol(c"opendir"));
    READDIR.set(symbol(c"readdir"));
    CLOSEDIR.set(symbol(c"closedir"));
    let write = |sites: &[Site]| sites.iter().try_for_each(|&(call, _, with)| core.call(call, 5, with));
    let mut parts = cx.parts();
    parts.named("listing cache", || {
        let sites = main_listing(core)?;
        *lock(&CACHE) = Some(Dirs::default());
        PENDING.set(None)?;
        write(&sites)
    });
    parts.named("prefix listing", || write(&prefix_listing(core)?));
    parts.finish()
}

pub fn print_stats() {
    if let Some(dirs) = lock(&CACHE).as_ref() {
        eprintln!(
            "dmfast: dir listings hits={} (without chdir {}) reads={} dirs={} verify={} mismatches={}; entries opened={} skipped={}",
            HITS.load(Relaxed),
            WITHOUT_CHDIR.load(Relaxed),
            READS.load(Relaxed),
            dirs.len(),
            verify(),
            super::verify::MISMATCHES.load(Relaxed),
            OPENED.load(Relaxed),
            SKIPPED.load(Relaxed)
        );
    }
}
