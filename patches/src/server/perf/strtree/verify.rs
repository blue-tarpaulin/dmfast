//! `DMFAST_VERIFY=1`: BYOND's tree stays linked (removal and the rebuilds run as shipped), every lookup also walks it
//! and answers with it, and an index answer that differs is a mismatch, as is a removal the index disagrees with.
//! While a .dmb load links the tree, and when a walk ends in a rebuild, answers aren't compared: the index is rebuilt
//! from the table afterwards.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use super::{FOUND, LEFT, RIGHT, STATE, STRING, State};
use crate::base::code::live_u32;
use crate::base::sync::lock;

/// The original walked into its rebuild, which links every entry.
static REBUILT: AtomicBool = AtomicBool::new(false);

impl State {
    /// [`State::sync`]'s verify part: index the entry the last miss's caller stored in the real link, and re-index
    /// the table after a load or a rebuild.
    pub(super) fn sync_real(&mut self, loading: bool) {
        if let Some(link) = self.pending.take() {
            // SAFETY: the real empty link the last miss handed out, which its caller filled.
            let stored = unsafe { live_u32(link) } as usize;
            if stored != 0 {
                self.insert(stored);
            }
        }
        let rebuilt = REBUILT.swap(false, Relaxed);
        // During a .dmb load only the loader looks up, and the load ends in a rebuild: compare from there.
        if loading {
            self.stale = true;
        } else if rebuilt || self.stale {
            self.stale = false;
            self.reindex();
        }
    }

    /// What a rebuild leaves in the tree: every entry with a non-empty string (entry 0 stays the root).
    fn reindex(&mut self) {
        self.clear();
        let f = FOUND.get();
        // SAFETY: the table's globals (install) and its live entries.
        unsafe {
            let (table, count) = (live_u32(f.table) as usize, live_u32(f.count) as usize);
            for i in 1..count {
                let e = live_u32(table + 4 * i) as usize;
                if e != 0 && *(live_u32(e + STRING) as *const u8) != 0 {
                    self.insert(e);
                }
            }
        }
    }
}

/// The real tree's answer to a lookup the index answered with `found`, counting a mismatch. Called unlocked: the
/// original's rebuild looks up again, and that inner lookup compares.
///
/// # Safety
/// BYOND's lookup arguments.
pub(super) unsafe fn lookup(key: *const *const u8, out: *mut usize, found: Option<usize>) -> usize {
    // SAFETY: BYOND's arguments (caller).
    let (real, rebuilt) = unsafe { walk(key, out) };
    let mut st = lock(&STATE);
    if !rebuilt && !st.stale && real != found.unwrap_or(0) {
        st.mismatches += 1;
    }
    if real == 0 {
        // SAFETY: the link the walk wrote.
        let link = unsafe { *out };
        st.pending = (link != 0).then_some(link);
    }
    real
}

/// The original walk: the entry for `key` or 0 (and its link in `out`), and whether the original answered after a
/// rebuild (past 1000 steps, unless the checksum global is -1).
///
/// # Safety
/// BYOND's lookup arguments.
unsafe fn walk(key: *const *const u8, out: *mut usize) -> (usize, bool) {
    let f = FOUND.get();
    // SAFETY: BYOND's arguments (caller), the table's globals (install), and the live entries linked from entry 0.
    unsafe {
        if live_u32(f.count) == 0 {
            *out = 0;
            return (0, false);
        }
        let s = *key;
        let mut e = live_u32(live_u32(f.table) as usize) as usize;
        let mut link = 0;
        let mut steps = 0u32;
        loop {
            let ord = compare(s, live_u32(e + STRING) as *const u8);
            if ord == Ordering::Equal {
                *out = link;
                return (e, false);
            }
            steps += 1;
            if steps > 1000 {
                if live_u32(f.checksum) != u32::MAX {
                    // The original rebuilds the tree from every entry, then looks up again (here).
                    REBUILT.store(true, Relaxed);
                    return (f.original.get()(key, out), true);
                }
                steps = 0;
            }
            link = e + if ord == Ordering::Greater { RIGHT } else { LEFT };
            e = live_u32(link) as usize;
            if e == 0 {
                *out = link;
                return (0, false);
            }
        }
    }
}

/// BYOND's compare: unsigned bytes up to the first NUL, stopping at the first difference.
///
/// # Safety
/// `a` and `b` are NUL-terminated.
unsafe fn compare(mut a: *const u8, mut b: *const u8) -> Ordering {
    // SAFETY: NUL-terminated (caller); stops at the NUL or earlier.
    unsafe {
        loop {
            let (x, y) = (*a, *b);
            if x != y || x == 0 {
                return x.cmp(&y);
            }
            a = a.add(1);
            b = b.add(1);
        }
    }
}

/// Removal, after BYOND's lookup found `found`: BYOND unlinks `entry` if it is `found`, and the index follows.
pub(super) extern "C" fn removed(entry: usize, found: usize) {
    let mut st = lock(&STATE);
    if st.remove(entry) != (found == entry) {
        st.mismatches += 1;
    }
}
