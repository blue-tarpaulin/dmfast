//! `DMFAST_VERIFY=1`: the stub runs every scan of a marked list and calls [`check`] first, which counts a key the
//! list already holds as a mismatch.

use crate::base::driver::Counter;

pub(super) static CHECKED: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

/// The new-key path's scan of `record`, a marked list, must not find (`key_type`, `key`). A list record holds its
/// items at +0 (8 bytes each: type byte at +0, data at +4) and its length at +0Ch.
///
/// # Safety
/// `record` is a live list record.
pub(super) unsafe extern "C" fn check(record: *const u32, key_type: u32, key: u32) {
    // SAFETY: a live list record (caller): `len` 8-byte items.
    let found = unsafe {
        let items = *record as *const u8;
        let len = *record.add(3) as usize;
        (0..len)
            .any(|i| *items.add(i * 8) == key_type as u8 && items.add(i * 8 + 4).cast::<u32>().read_unaligned() == key)
    };
    CHECKED.bump();
    if found {
        MISMATCHES.bump();
    }
}
