//! `DMFAST_VERIFY=1`: every intern also runs the original, whose id is used. It's a mismatch if the original returns
//! another id for an index hit, appends for a hit, or finds an entry for a miss.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::{RESERVED, State, original};
use crate::base::code::live_u32;
use crate::base::driver::verify;

pub(super) static MISMATCHES: AtomicU32 = AtomicU32::new(0);

/// With verify on, the original's id for `list`, given the index's answer `hit` (`None` for a miss).
///
/// # Safety
/// As [`super::intern`]: `list` is the compiler's non-empty initializer.
pub(super) unsafe fn intern(st: &mut State, list: usize, hit: Option<u32>) -> Option<u32> {
    if !verify() {
        return None;
    }
    // SAFETY: the count global was checked at install.
    let count = unsafe { live_u32(st.found.count) };
    let id = unsafe { original(st, list) };
    let appended = unsafe { live_u32(st.found.count) } != count;
    let expected = match hit {
        Some(h) => !appended && id == h,
        None => appended && id == if count == RESERVED { RESERVED + 1 } else { count },
    };
    if !expected {
        MISMATCHES.fetch_add(1, Relaxed);
    }
    Some(id)
}
