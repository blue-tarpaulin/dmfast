//! `mapskip`: SendMaps' per-movable record check without its temporary record when nothing changed.
//!
//! Every tick, per client, SendMaps checks each movable in view against the client's record tree (red-black, keyed by
//! object id), building a temporary record to look the id up (inserting it if missing). The platform stub, hooked at
//! the check's entry, takes only the unchanged path (the record holds the object's appearance and next id): a
//! read-only descent and two compares, then it sets the seen flag and returns `flags & MASK`. Anything else runs the
//! original. It's safe because on that path the original writes nothing else: its temporary's destructor releases
//! appearance none (FFFFh), which returns at once. `DMFAST_VERIFY=1` hooks [`verify::check`] instead.

use crate::base::feature::{Feature, Hosts, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static MAPSKIP: Feature = Feature::perf(
    Hosts::SERVER,
    "mapskip",
    "unchanged map movables",
    install!(windows: windows::install, linux: linux::install),
)
.stats(print_stats);

/// Record tree layout (pinned by each platform's signatures): the root at tree +0; a record's left and right
/// children, key (object id, unsigned), next id, and appearance. Its flags and the seen flag:
/// [`crate::server::layout`].
const LEFT: usize = 0x00;
const RIGHT: usize = 0x04;
const KEY: usize = 0x1C;
const NEXT: usize = 0x2C;
const LOOK: usize = 0x30;
/// The mask the unchanged path returns the flags through (after setting the seen flag).
const MASK: u32 = 0xDCF;

/// Set when the install commits.
static INSTALLED: Installed<()> = Installed::new();

fn print_stats() {
    if INSTALLED.is_set() {
        eprintln!(
            "dmfast: mapskip checked={} unchanged={} mismatches={}",
            verify::CHECKED,
            verify::UNCHANGED,
            verify::MISMATCHES
        );
    }
}
