//! `DMFAST_VERIFY=1`: each cache hit re-reads its directory and compares.

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::linux::{Entry, Stat, change_now, read_listing, stat};
use crate::base::driver::verify;

pub(super) static MISMATCHES: AtomicU32 = AtomicU32::new(0);

/// With verify on, re-read `path` (the cached directory) and count a mismatch if its entries differ while its stamp
/// is still `st`'s. A directory the compiler changed after the stamp check isn't stale: the listing was current then.
pub(super) fn hit(path: *const c_char, listing: &[Entry], st: Option<Stat>) {
    if !verify() {
        return;
    }
    let stale = || {
        !read_listing(path).is_some_and(|fresh| same_entries(&fresh, listing))
            && stat(path).map(|s| s.stamp) == st.map(|s| s.stamp)
    };
    if !change_now() || stale() {
        MISMATCHES.fetch_add(1, Relaxed);
    }
}

fn same_entries(a: &[Entry], b: &[Entry]) -> bool {
    // SAFETY: copied names are NUL-terminated.
    let name = |e: &Entry| unsafe { CStr::from_ptr(e.dirent.d_name.as_ptr()) };
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.dirent.d_type == y.dirent.d_type && name(x) == name(y))
}
