//! `DMFAST_VERIFY_STRTABLE=1`: after each pass, the index is set aside, BYOND's own pass and rebuild run over the
//! same table, and every entry's fields and the index they leave must match ours. Separate from `DMFAST_VERIFY=1`,
//! under which this feature doesn't install.

use super::windows::{FOUND, Found};
use crate::base::code::{live_u32, read};
use crate::base::driver::Counter;
use crate::server::perf::strtree::{SITES, index_matches, take_index};

static CHECKED: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// Each entry's first seven dwords (zeros when absent): string, id, links, reference count, CRC, and length.
///
/// # Safety
/// The table is loaded.
unsafe fn fields() -> Vec<[u32; 7]> {
    let s = SITES.get();
    // SAFETY: the loaded table (caller): `count` entry pointers, each 0 or a live entry.
    unsafe {
        let (table, count) = (live_u32(s.table) as usize, live_u32(s.count) as usize);
        (0..count)
            .map(|i| match live_u32(table + 4 * i) as usize {
                0 => [0; 7],
                e => read(e),
            })
            .collect()
    }
}

/// Runs BYOND's pass after ours and compares.
///
/// # Safety
/// As the pass: called by the loader, the table loaded.
pub(super) unsafe fn check(f: &Found) {
    // SAFETY: as the pass (caller); the original is its trampoline.
    let same = unsafe {
        let ours = fields();
        let index = take_index();
        f.original.get()();
        ours == fields() && index_matches(&index)
    };
    CHECKED.bump();
    if !same {
        MISMATCHES.bump();
    }
    eprintln!("dmfast: string table load verify: {}", if same { "identical" } else { "MISMATCH" });
}

pub(super) fn print_stats() {
    if FOUND.is_set() && FOUND.get().check {
        eprintln!("dmfast: string table load verify checked={CHECKED} mismatches={MISMATCHES}");
    }
}
