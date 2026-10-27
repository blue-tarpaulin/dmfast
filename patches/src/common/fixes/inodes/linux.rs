//! `inodes` (forum report 2991326): on filesystems with 64-bit inode numbers (WSL's `/mnt/c`, some NFS/XFS/btrfs),
//! 32-bit `stat` and `readdir` fail with `EOVERFLOW`, so DreamMaker opens no .dme. The `stat`, `fstat` and `readdir`
//! below forward to glibc's until the fix is on, then call the 64-bit versions and fold inodes into 32 bits
//! (libbyond never reads them, so collisions are harmless).

use std::ffi::{CStr, c_char, c_int};
use std::mem::{MaybeUninit, offset_of};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use libc::{__errno_location, DIR, EOVERFLOW, RTLD_NEXT, dirent, dirent64, dlsym, stat as Stat, stat64};

use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::linux::symbol;
use crate::base::{Result, bail};

static ON: AtomicBool = AtomicBool::new(false);

type StatFn = unsafe extern "C" fn(*const c_char, *mut Stat) -> c_int;
type FstatFn = unsafe extern "C" fn(c_int, *mut Stat) -> c_int;
type ReaddirFn = unsafe extern "C" fn(*mut DIR) -> *mut dirent;

static GLIBC_STAT: Foreign<StatFn> = Foreign::new();
static GLIBC_FSTAT: Foreign<FstatFn> = Foreign::new();
static GLIBC_READDIR: Foreign<ReaddirFn> = Foreign::new();

/// glibc's `name` while the fix is off; `None` once it is on, or if there is none (then we convert anyway).
fn glibc<F: Copy>(f: &Foreign<F>, name: &CStr) -> Option<F> {
    if ON.load(Relaxed) {
        return None;
    }
    if f.addr() == 0 {
        // SAFETY: a lookup by NUL-terminated name.
        f.set(unsafe { dlsym(RTLD_NEXT, name.as_ptr()) } as usize);
    }
    // SAFETY: `dlsym` found glibc's `name`, which has `F`'s signature.
    (f.addr() != 0).then(|| unsafe { f.get() })
}

/// The halves xor-ed, so inodes that differ only in the high half stay distinct.
fn ino32(ino: u64) -> u32 {
    (ino as u32) ^ ((ino >> 32) as u32)
}

/// The result of a 64-bit `stat` call (`r`, `s`) as the 32-bit call's into `out`.
///
/// # Safety
/// `s` is filled if `r` is 0; `out` is writable.
unsafe fn narrow(r: c_int, s: &MaybeUninit<stat64>, out: *mut Stat) -> c_int {
    if r != 0 {
        return -1;
    }
    // SAFETY: filled (caller).
    let s = unsafe { s.assume_init_ref() };
    let (Ok(size), Ok(blocks)) = (s.st_size.try_into(), s.st_blocks.try_into()) else {
        // SAFETY: this thread's errno.
        unsafe { *__errno_location() = EOVERFLOW };
        return -1;
    };
    // SAFETY: `stat` is plain integers (padding included), so all zeroes is a valid value.
    let mut o: Stat = unsafe { std::mem::zeroed() };
    o.st_dev = s.st_dev;
    o.st_ino = ino32(s.st_ino);
    o.st_mode = s.st_mode;
    o.st_nlink = s.st_nlink;
    o.st_uid = s.st_uid;
    o.st_gid = s.st_gid;
    o.st_rdev = s.st_rdev;
    o.st_size = size;
    o.st_blksize = s.st_blksize;
    o.st_blocks = blocks;
    o.st_atime = s.st_atime as _;
    o.st_atime_nsec = s.st_atime_nsec as _;
    o.st_mtime = s.st_mtime as _;
    o.st_mtime_nsec = s.st_mtime_nsec as _;
    o.st_ctime = s.st_ctime as _;
    o.st_ctime_nsec = s.st_ctime_nsec as _;
    // SAFETY: writable (caller).
    unsafe { out.write(o) };
    0
}

/// # Safety
/// As libc's `stat`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stat(path: *const c_char, buf: *mut Stat) -> c_int {
    let mut s = MaybeUninit::uninit();
    // SAFETY: glibc's `stat` or `stat64` with the caller's arguments; `narrow` gets what `stat64` filled.
    unsafe {
        match glibc(&GLIBC_STAT, c"stat") {
            Some(f) => f(path, buf),
            None => narrow(libc::stat64(path, s.as_mut_ptr()), &s, buf),
        }
    }
}

/// # Safety
/// As libc's `fstat`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fstat(fd: c_int, buf: *mut Stat) -> c_int {
    let mut s = MaybeUninit::uninit();
    // SAFETY: glibc's `fstat` or `fstat64` with the caller's arguments; `narrow` gets what `fstat64` filled.
    unsafe {
        match glibc(&GLIBC_FSTAT, c"fstat") {
            Some(f) => f(fd, buf),
            None => narrow(libc::fstat64(fd, s.as_mut_ptr()), &s, buf),
        }
    }
}

// A converted entry fits in the record it replaces.
const _: () = assert!(offset_of!(dirent, d_name) < offset_of!(dirent64, d_name));

/// # Safety
/// As libc's `readdir`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readdir(dir: *mut DIR) -> *mut dirent {
    // SAFETY: glibc's `readdir` or `readdir64` with the caller's stream. A `readdir64` record stays valid until the
    // next read, and is rewritten in place as the shorter 32-bit one (as glibc did before 2.37).
    unsafe {
        if let Some(f) = glibc(&GLIBC_READDIR, c"readdir") {
            return f(dir);
        }
        let e = libc::readdir64(dir);
        if e.is_null() {
            return e.cast();
        }
        let name = (&raw const (*e).d_name).cast::<c_char>();
        let len = CStr::from_ptr(name).count_bytes();
        let (ino, off, d_type) = ((*e).d_ino, (*e).d_off, (*e).d_type);
        let out = e.cast::<dirent>();
        std::ptr::copy(name, (&raw mut (*out).d_name).cast::<c_char>(), len + 1);
        (*out).d_ino = ino32(ino);
        (*out).d_off = off as _;
        (*out).d_reclen = (offset_of!(dirent, d_name) + len + 1).next_multiple_of(4) as u16;
        (*out).d_type = d_type;
        out
    }
}

/// On once every lookup finds our functions (the preload works) and libbyond imports `stat`.
pub fn install(cx: &Ctx) -> Result<String> {
    for (name, ours) in [
        (c"stat", stat as StatFn as usize),
        (c"fstat", fstat as FstatFn as usize),
        (c"readdir", readdir as ReaddirFn as usize),
    ] {
        if symbol(name) != ours {
            bail!("{} does not resolve to libdmfast.so (not preloaded?)", name.to_string_lossy());
        }
    }
    for name in [c"stat64", c"fstat64", c"readdir64"] {
        if symbol(name) == 0 {
            bail!("no {}", name.to_string_lossy());
        }
    }
    // `__xstat`/`lstat` imports aren't covered. readdir's call sites aren't checked: `dirs` rewrites them to calls
    // through the lookup checked above.
    if cx.core().import_calls("stat", stat as StatFn as usize)?.is_empty() {
        bail!("libbyond does not import stat");
    }
    ON.store(true, Relaxed);
    Ok("64-bit inode numbers in stat, fstat and readdir".into())
}
