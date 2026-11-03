//! `DMFAST_VERIFY=1`: on every call, BYOND's walk and the sweep run from the same flags; every slot's flags must come
//! out the same, and the list must hold exactly the in-use slots. The sweep's result stays.

use std::cell::RefCell;

use super::{FIRST, NEXT, each_slot, sweep};
use crate::base::code::{read, write};
use crate::base::driver::Counter;
use crate::server::layout::{RECORD_FLAGS, SEEN};

pub(super) static CALLS: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

thread_local! {
    /// Every slot's flags before, and after BYOND's walk.
    static FLAGS: RefCell<(Vec<u16>, Vec<u16>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// BYOND's clear: the list in id order. Returns its length.
///
/// # Safety
/// `tree` is a live record tree.
unsafe fn walk(tree: usize) -> usize {
    // SAFETY: a live tree (caller): its list of live nodes.
    unsafe {
        let mut n = 0;
        let mut node = read::<u32>(tree + FIRST) as usize;
        while node != 0 {
            write(node + RECORD_FLAGS, read::<u16>(node + RECORD_FLAGS) & !SEEN);
            n += 1;
            node = read::<u32>(node + NEXT) as usize;
        }
        n
    }
}

/// [`sweep`], checked against [`walk`].
///
/// # Safety
/// `tree` is a live record tree only this thread uses.
pub(super) unsafe fn clear(tree: usize) {
    CALLS.bump();
    // SAFETY: a live tree (caller); the slots are the same on every pass.
    unsafe {
        FLAGS.with_borrow_mut(|(before, walked)| {
            before.clear();
            walked.clear();
            let mut used = 0;
            each_slot(tree, true, |node, u| {
                before.push(read(node + RECORD_FLAGS));
                used += usize::from(u);
            });
            let listed = walk(tree);
            each_slot(tree, true, |node, _| walked.push(read(node + RECORD_FLAGS)));
            let mut saved = before.iter();
            each_slot(tree, true, |node, _| {
                if let Some(&flags) = saved.next() {
                    write(node + RECORD_FLAGS, flags);
                }
            });
            sweep(tree);
            let mut expected = walked.iter();
            let mut same = listed == used;
            each_slot(tree, true, |node, _| same &= expected.next() == Some(&read::<u16>(node + RECORD_FLAGS)));
            if !same {
                MISMATCHES.bump();
            }
        });
    }
}
