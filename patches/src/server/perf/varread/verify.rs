//! `DMFAST_VERIFY=1`: every answered read also runs the original read; a different type byte or data is a mismatch.

use super::cache::{State, Value};
use super::windows::original;
use crate::base::driver::verify;

/// With verify on, compare `mine`, the answer for var `name` of (`tag`, `id`), with the original's.
///
/// # Safety
/// As [`super::cache::read`]: BYOND's arguments to a read that the hook answered (so the original raises no error).
#[inline(always)]
pub(super) unsafe fn check(st: &mut State, tag: u32, id: u32, name: u32, mine: Value) {
    if !verify() {
        return;
    }
    // SAFETY: the original read, with the arguments the hook got (caller).
    let r = unsafe { original(tag, id, name) };
    let c = &mut st.counters;
    c.checked += 1;
    if r.tag & 0xFF == mine.tag & 0xFF && r.data == mine.data {
        return;
    }
    c.mismatches += 1;
    // The first few are printed.
    if c.mismatches <= 10 {
        eprintln!(
            "dmfast: varread mismatch: value {tag:#x}:{id:#x} var {:#x}: byond {:#x}:{:#x}, ours {:#x}:{:#x}",
            name & 0xFF_FFFF,
            r.tag,
            r.data,
            mine.tag,
            mine.data
        );
    }
}
