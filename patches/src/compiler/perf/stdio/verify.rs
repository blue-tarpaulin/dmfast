//! `DMFAST_VERIFY=1`: each `ftell` answered from a stream's state is compared with the CRT's, and on Windows each
//! fast-path call is checked to come from its stream's first thread.

#[cfg(windows)]
use std::sync::Mutex;

use crate::base::driver::Counter;
#[cfg(windows)]
use crate::base::hash::{self, Map};
#[cfg(windows)]
use crate::base::sync::{lock, thread_id};

/// `ftell` answers compared with the CRT's, and how many disagreed.
pub(super) static CHECKS: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

/// Count the comparison of the answer `pos` with the CRT's `real`.
pub(super) fn compare<T: PartialEq>(pos: T, real: T) {
    CHECKS.bump();
    if pos != real {
        MISMATCHES.bump();
    }
}

/// Each stream's (`FILE` address's) first thread.
#[cfg(windows)]
static OWNERS: Mutex<Map<usize, usize>> = Mutex::new(hash::map());
/// Fast-path calls checked, and those from a thread other than their stream's first.
#[cfg(windows)]
pub(super) static CALLS: Counter = Counter::new();
#[cfg(windows)]
pub(super) static OTHER_THREAD: Counter = Counter::new();

/// Check a fast-path call on stream `fp`.
#[cfg(windows)]
pub(super) fn check_thread(fp: usize) {
    let tid = thread_id();
    let owner = *lock(&OWNERS).entry(fp).or_insert(tid);
    CALLS.bump();
    if owner != tid {
        OTHER_THREAD.bump();
    }
}
