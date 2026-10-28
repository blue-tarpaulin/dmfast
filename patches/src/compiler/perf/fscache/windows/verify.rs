//! `DMFAST_VERIFY=1`: each answer from a listing is checked against the real call, and the real result is used (and
//! counted as a mismatch) when they differ.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Storage::FileSystem::WIN32_FIND_DATAW;

use super::listing::Counters;
use super::{Find, Stat, wslice};
use crate::base::driver::verify;

/// `ans`, or with verify on, `real()` if it differs.
pub(super) fn find(ans: Find, real: impl FnOnce() -> Find, counters: &mut Counters) -> Find {
    checked(ans, real, counters, |a, r| a.ok == r.ok && if r.ok { same_find(&a.data, &r.data) } else { a.err == r.err })
}

/// `ans`, or with verify on, `real()` if it differs. A failed stat compares only its errno.
pub(super) fn stat(ans: Stat, real: impl FnOnce() -> Stat, counters: &mut Counters) -> Stat {
    let key = |x: &Stat| if x.0 == 0 { (0, x.1, 0) } else { (x.0, [0; 48], x.2) };
    checked(ans, real, counters, |a, r| key(a) == key(r))
}

fn checked<T>(ans: T, real: impl FnOnce() -> T, counters: &mut Counters, same: impl Fn(&T, &T) -> bool) -> T {
    if !verify() {
        return ans;
    }
    let r = real();
    if same(&ans, &r) {
        ans
    } else {
        counters.mismatches += 1;
        r
    }
}

/// The fields of a find result byondcore can observe.
fn same_find(a: &WIN32_FIND_DATAW, b: &WIN32_FIND_DATAW) -> bool {
    let t = |f: FILETIME| (f.dwLowDateTime, f.dwHighDateTime);
    // SAFETY: the names of a find result are NUL-terminated.
    unsafe {
        a.dwFileAttributes == b.dwFileAttributes
            && t(a.ftCreationTime) == t(b.ftCreationTime)
            && t(a.ftLastAccessTime) == t(b.ftLastAccessTime)
            && t(a.ftLastWriteTime) == t(b.ftLastWriteTime)
            && (a.nFileSizeHigh, a.nFileSizeLow, a.dwReserved0) == (b.nFileSizeHigh, b.nFileSizeLow, b.dwReserved0)
            && wslice(a.cFileName.as_ptr()) == wslice(b.cFileName.as_ptr())
            && wslice(a.cAlternateFileName.as_ptr()) == wslice(b.cAlternateFileName.as_ptr())
    }
}
