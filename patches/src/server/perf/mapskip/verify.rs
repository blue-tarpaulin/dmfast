//! `DMFAST_VERIFY=1`: every check runs the original; where the stub would have answered, the record and the result
//! must come out as the stub leaves them.

use super::{KEY, LEFT, LOOK, MASK, NEXT, RIGHT};
use crate::base::code::read;
use crate::base::driver::Counter;
use crate::server::layout::{RECORD_FLAGS, SEEN};

pub(super) static CHECKED: Counter = Counter::new();
pub(super) static UNCHANGED: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

/// Bytes of a record the initializer writes (its last field is the byte at +5Ah): what is compared.
const RECORD: usize = 0x5B;

/// The record the stub takes the unchanged path on, found as the stub finds it.
///
/// # Safety
/// `tree` is a live record tree.
unsafe fn unchanged(tree: usize, id: u32, look: u32, next: u32) -> Option<*mut u8> {
    // SAFETY: a live tree (caller): records linked through their children.
    unsafe {
        let mut node = read::<usize>(tree);
        while node != 0 {
            let key = read::<u32>(node + KEY);
            node = if id < key {
                read(node + LEFT)
            } else if id > key {
                read(node + RIGHT)
            } else {
                let hit = read::<u32>(node + LOOK) == look && read::<u32>(node + NEXT) == next;
                return hit.then_some(node as *mut u8);
            };
        }
        None
    }
}

/// Runs `original` and, where the stub would have answered, checks the record and the result against it.
///
/// # Safety
/// As the check's call: `tree` is a live record tree, `look` the object's appearance.
pub(super) unsafe fn check(tree: usize, id: u32, look: u32, next: u32, original: impl FnOnce() -> u32) -> u32 {
    CHECKED.bump();
    // SAFETY: a live tree (caller); `node` is one of its records, `RECORD` bytes long.
    unsafe {
        let Some(node) = unchanged(tree, id, look, next) else { return original() };
        UNCHANGED.bump();
        let mut expected = [0u8; RECORD];
        std::ptr::copy_nonoverlapping(node, expected.as_mut_ptr(), RECORD);
        let flags = u16::from_le_bytes([expected[RECORD_FLAGS], expected[RECORD_FLAGS + 1]]) | SEEN;
        expected[RECORD_FLAGS..RECORD_FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
        let result = original();
        if result != flags as u32 & MASK || std::slice::from_raw_parts(node, RECORD) != expected {
            MISMATCHES.bump();
        }
        result
    }
}
