//! `DMFAST_VERIFY=1`: the frame values the stub reads must agree with the scan they describe.

use std::cell::Cell;

use crate::base::driver::Counter;

pub(super) static CHECKED: Counter = Counter::new();
/// Calls that follow the previous one's scan, so [`check`] could predict their turfs left.
pub(super) static FOLLOWED: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

thread_local! {
    /// The previous call on this thread (map sending can run on several): (frame, turf, turfs left in its row, row
    /// width, rows left).
    static LAST: Cell<(usize, u32, u32, u32, u32)> = const { Cell::new((0, 0, 0, 0, 0)) };
}

/// `rem` is below the row width, and follows from the previous call's on this thread in the same frame when `t` is
/// further along that scan (same row: fewer by the distance; next row, one fewer left: from its start, `maxx - width`
/// past the previous row's end).
pub(super) fn check(maxx: u32, t: u32, frame: usize, rem: u32, width: u32, rows: u32) {
    let (pf, pt, prem, pwidth, prows) = LAST.with(|l| l.replace((frame, t, rem, width, rows)));
    let start = pt.wrapping_add(prem).wrapping_add(1).wrapping_add(maxx.wrapping_sub(width));
    let want = if pf != frame || pwidth != width || t <= pt {
        None
    } else if rows == prows && t - pt <= prem {
        Some(prem - (t - pt))
    } else if rows == prows.wrapping_sub(1) & 0xFFFF && t >= start && t - start < width {
        Some(width - 1 - (t - start))
    } else {
        None
    };
    CHECKED.bump();
    if want.is_some() {
        FOLLOWED.bump();
    }
    if rem >= width || want.is_some_and(|w| w != rem) {
        MISMATCHES.bump();
    }
}
