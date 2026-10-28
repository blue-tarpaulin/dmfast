//! Directory listings behind the cache. Each directory is listed once, with `FindFirstFileExW(dir\*)` at the
//! information level `FindFirstFileW` uses, and a question about one name in it is answered from the listing:
//! - the name is there with exactly the requested spelling (long or 8.3): that entry;
//! - the name isn't there: not found, unless the directory has non-ASCII names, whose case folding isn't modeled;
//! - anything else (a case-only match, wildcards, `.` or `..`, trailing dots or spaces, UNC, drive-relative, or long
//!   paths, or names that collide once lowercased): undecided, and the caller makes the real call.
//!
//! Listings follow the cache's rules: unmodified directories only, dropped on any modification, owner thread only.

use rustc_hash::FxHashMap;
use windows_sys::Win32::Foundation::{FILETIME, INVALID_HANDLE_VALUE, MAX_PATH, SYSTEMTIME};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
    FindClose, FindExInfoStandard, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use super::{is_volatile, normalize, wslice};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::windows::ucrt::ucrt_export;

struct Listing {
    by_name: FxHashMap<Box<[u16]>, WIN32_FIND_DATAW>,
    non_ascii: bool,
}

#[derive(Default)]
pub(super) struct Counters {
    pub find: u64,
    pub stat: u64,
    pub lists: u64,
    pub mismatches: u64,
}

/// Each directory's listing (`None` if it can't answer), by normalized path.
#[derive(Default)]
pub(super) struct Listings {
    dirs: FxHashMap<Box<[u16]>, Option<Listing>>,
    pub counters: Counters,
}

fn lower(s: &[u16]) -> Box<[u16]> {
    s.iter().map(|&c| if (0x41..=0x5A).contains(&c) { c + 0x20 } else { c }).collect()
}

fn list(dir: &[u16], counters: &mut Counters) -> Option<Listing> {
    let pattern: Vec<u16> = dir.iter().copied().chain([0x5C, 0x2A, 0]).collect();
    let mut by_name = FxHashMap::default();
    let mut non_ascii = false;
    let mut ok = true;
    unsafe {
        let mut d: WIN32_FIND_DATAW = std::mem::zeroed();
        let h = FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoStandard,
            (&raw mut d).cast(),
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        );
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        loop {
            let long = wslice(d.cFileName.as_ptr());
            let short = wslice(d.cAlternateFileName.as_ptr());
            non_ascii |= long.iter().chain(short).any(|&c| c >= 0x80);
            if long != [0x2E] && long != [0x2E, 0x2E] {
                for n in [long, short] {
                    if !n.is_empty() && by_name.insert(lower(n), d).is_some() {
                        ok = false; // names collide after lowercasing: case-sensitive directory
                    }
                }
            }
            // FindNextFileW keeps unset fields (dwReserved0) of the last entry; a lone FindFirstFileW zeroes them.
            d = std::mem::zeroed();
            if FindNextFileW(h, &mut d) == 0 {
                break;
            }
        }
        FindClose(h);
    }
    counters.lists += 1;
    ok.then_some(Listing { by_name, non_ascii })
}

/// A plain final path component: printable ASCII, no wildcards/reserved chars, not `.`/`..`, no trailing dot/space.
fn plain(name: &[u16]) -> bool {
    !name.is_empty()
        && name.iter().all(|&c| (0x20..0x7F).contains(&c) && !b"*?\"<>|:\\/".contains(&(c as u8)))
        && !matches!(name.last(), Some(0x2E | 0x20))
}

impl Listings {
    pub(super) fn clear(&mut self) {
        self.dirs.clear();
    }

    /// What the listing of `path`'s directory says: `Some((Some(entry), drive))` / `Some((None, drive))` for present /
    /// absent, `None` if undecidable. `drive` is the uppercase drive letter.
    pub(super) fn lookup(&mut self, path: &[u16]) -> Option<(Option<WIN32_FIND_DATAW>, u16)> {
        // `D:foo` and `\foo` depend on per-drive current directories: never.
        let absolute = matches!(path, [_, 0x3A, 0x5C | 0x2F, ..]);
        if !absolute && matches!(path, [_, 0x3A, ..] | [0x5C | 0x2F, ..]) {
            return None;
        }
        let raw = &path[path.iter().rposition(|&c| c == 0x5C || c == 0x2F).map_or(0, |i| i + 1)..];
        if !plain(raw) {
            return None;
        }
        let full = normalize(path);
        if full.len() >= MAX_PATH as usize
            || full.len() < 4
            || full[1] != 0x3A
            || full[2] != 0x5C
            || !(0x61..=0x7A).contains(&full[0])
        {
            return None;
        }
        let i = full.iter().rposition(|&c| c == 0x5C)?;
        let (dir, name) = (&full[..i.max(2)], &full[i + 1..]);
        if *name != *lower(raw) || is_volatile(&full) || is_volatile(dir) {
            return None;
        }
        let counters = &mut self.counters;
        let listing = self.dirs.entry(dir.into()).or_insert_with(|| list(dir, counters)).as_ref()?;
        let drive = full[0] - 0x20;
        match listing.by_name.get(name) {
            Some(e) => {
                // SAFETY: the names of a find result are NUL-terminated.
                let exact =
                    unsafe { wslice(e.cFileName.as_ptr()) == raw || wslice(e.cAlternateFileName.as_ptr()) == raw };
                exact.then_some((Some(*e), drive))
            }
            None => (!listing.non_ascii).then_some((None, drive)),
        }
    }
}

#[repr(C)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}

/// ucrtbase's `_mktime64`, whose time zone rules are the ones byondcore's `_wstat64i32` applies.
static MKTIME64: Foreign<unsafe extern "C" fn(*mut Tm) -> i64> = Foreign::new();

/// Resolves what [`stat_of`] calls; before the stat reader is hooked.
pub(super) fn resolve() -> Result<()> {
    MKTIME64.set(ucrt_export("_mktime64")?);
    Ok(())
}

/// The CRT's stat time: UTC -> local wall time by the time zone rules for that date -> time_t.
fn stat_time(ft: FILETIME) -> Option<i64> {
    if ft.dwLowDateTime == 0 && ft.dwHighDateTime == 0 {
        return Some(0);
    }
    unsafe {
        let (mut utc, mut local) = (std::mem::zeroed::<SYSTEMTIME>(), std::mem::zeroed::<SYSTEMTIME>());
        if FileTimeToSystemTime(&ft, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return None;
        }
        let mut tm = Tm {
            sec: i32::from(local.wSecond),
            min: i32::from(local.wMinute),
            hour: i32::from(local.wHour),
            mday: i32::from(local.wDay),
            mon: i32::from(local.wMonth) - 1,
            year: i32::from(local.wYear) - 1900,
            wday: 0,
            yday: 0,
            isdst: -1,
        };
        let t = MKTIME64.get()(&mut tm);
        (t != -1).then_some(t)
    }
}

/// `struct _stat64i32` as `_wstat64i32` fills it for a plain file; `None` for anything it would treat
/// differently (directories, reparse points, sizes that overflow `st_size`).
pub(super) fn stat_of(e: &WIN32_FIND_DATAW, drive: u16, path: &[u16]) -> Option<[u8; 48]> {
    let size = (u64::from(e.nFileSizeHigh) << 32) | u64::from(e.nFileSizeLow);
    if e.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 || size > 0x7FFF_FFFF {
        return None;
    }
    let mut mode: u16 = 0x8000 | if e.dwFileAttributes & FILE_ATTRIBUTE_READONLY != 0 { 0x100 } else { 0x180 };
    let ext = lower(&path[path.len().saturating_sub(4)..]);
    if [".exe", ".cmd", ".bat", ".com"].iter().any(|x| x.encode_utf16().eq(ext.iter().copied())) {
        mode |= 0x40;
    }
    mode |= ((mode & 0x1C0) >> 3) | ((mode & 0x1C0) >> 6);
    let dev = u32::from(drive - 0x41);
    let mut b = [0u8; 48];
    b[0..4].copy_from_slice(&dev.to_le_bytes());
    b[6..8].copy_from_slice(&mode.to_le_bytes());
    b[8..10].copy_from_slice(&1i16.to_le_bytes());
    b[16..20].copy_from_slice(&dev.to_le_bytes());
    b[20..24].copy_from_slice(&(size as i32).to_le_bytes());
    b[24..32].copy_from_slice(&stat_time(e.ftLastAccessTime)?.to_le_bytes());
    b[32..40].copy_from_slice(&stat_time(e.ftLastWriteTime)?.to_le_bytes());
    b[40..48].copy_from_slice(&stat_time(e.ftCreationTime)?.to_le_bytes());
    Some(b)
}
