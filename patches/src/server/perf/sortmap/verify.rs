//! `DMFAST_VERIFY=1`: each merged map is compared with the union BYOND's merge produces (the destination's entries,
//! then per source entry with values its key added and its values appended); a difference is a mismatch.

use std::collections::BTreeMap;

use super::{Map, at, count_of, key_of, values_of};
use crate::base::driver::{Counter, verify};

pub(super) static CHECKED: Counter = Counter::new();
pub(super) static MISMATCHES: Counter = Counter::new();

/// A map's entries: key, flag byte, and values.
type Snapshot = Vec<(u32, u8, Vec<u32>)>;

/// # Safety
/// `m` is a live map.
unsafe fn snapshot(m: &Map) -> Snapshot {
    // SAFETY: a live map (caller): its entries and their value arrays.
    unsafe {
        (0..m.count as usize)
            .map(|i| {
                let e = at(m, i);
                let v = values_of(e) as *const u32;
                let n = count_of(e) as usize;
                (key_of(e), *e.add(10), if v.is_null() { vec![] } else { std::slice::from_raw_parts(v, n).to_vec() })
            })
            .collect()
    }
}

/// With verify on, both maps before the merge.
///
/// # Safety
/// `dst` and `src` are live maps.
pub(super) unsafe fn before(dst: &Map, src: &Map) -> Option<(Snapshot, Snapshot)> {
    // SAFETY: live maps (caller).
    verify().then(|| unsafe { (snapshot(dst), snapshot(src)) })
}

/// Compares the merged map with what [`before`] saw.
///
/// # Safety
/// `got` is a live map.
pub(super) unsafe fn check(before: Option<(Snapshot, Snapshot)>, got: &Map) {
    let Some((dst, src)) = before else { return };
    let mut want: BTreeMap<u32, (u8, Vec<u32>)> = dst.into_iter().map(|(k, f, v)| (k, (f, v))).collect();
    for (k, f, v) in src.into_iter().filter(|(_, _, v)| !v.is_empty()) {
        want.entry(k).or_insert((f, vec![])).1.extend(v);
    }
    let want: Snapshot = want.into_iter().map(|(k, (f, v))| (k, f, v)).collect();
    CHECKED.bump();
    // SAFETY: a live map (caller).
    if unsafe { snapshot(got) } != want {
        MISMATCHES.bump();
    }
}
