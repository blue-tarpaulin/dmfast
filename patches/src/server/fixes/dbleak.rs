//! `dbleak`: `/database/query`'s `GetRowData()` and `Columns()` no longer leak the lists they return.
//!
//! Both build a new list, which starts with one reference, and hand it back as a builtin result, which the
//! interpreter references again when it pushes it. The creation's reference is never let go, so each list stays
//! forever with what it holds. The fix lets go of it right after creation, as BYOND does for the lists it caches.
//!
//! Safe: the list keeps the interpreter's reference; only the extra count goes.

use crate::base::code::{self, Foreign};
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

pub static DBLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "dbleak",
    "GetRowData() and Columns() lists don't keep their creation reference",
    Tier::Safe,
    install!(windows: install, linux: install),
);

/// In `Columns()` and `GetRowData()`: call create ; ... (make the list value) ; push the list id ; ... ;
/// call lookup (the list's record by id).
#[cfg(windows)]
const BUILDERS: [(&str, usize, usize); 2] = [
    ("8B D8 53 E8 ?? ?? ?? ?? 8B F0 6A 0F 56 E8 ?? ?? ?? ?? 56 89 45 F8 89 55 F4 E8", 3, 25),
    ("8B CB 89 5D F0 0F 45 C8 51 E8 ?? ?? ?? ?? 8B F0 6A 0F 56 E8 ?? ?? ?? ?? 56 89 45 FC 89 55 F8 E8", 9, 31),
];
#[cfg(target_os = "linux")]
const BUILDERS: [(&str, usize, usize); 2] = [
    (
        "89 04 24 89 C3 E8 ?? ?? ?? ?? 83 C4 0C 6A 0F 89 C6 50 55 E8 ?? ?? ?? ?? F3 0F 7E 44 24 24 0F 29 44 24 0C 56 E8",
        5,
        36,
    ),
    (
        "6A 00 E8 ?? ?? ?? ?? 8D 74 24 48 83 C4 0C 6A 0F 89 C5 50 56 E8 ?? ?? ?? ?? F3 0F 7E 44 24 44 0F 29 44 24 2C 55 E8",
        2,
        37,
    ),
];
/// A list record's reference count.
const REFS: usize = 0x10;

type Lookup = unsafe extern "C" fn(u32) -> usize;

static LOOKUP: Installed<Foreign<Lookup>> = Installed::new();

/// In place of the builders' lookup of the list they just made: the lookup, minus the creation's reference.
extern "C" fn fresh(id: u32) -> usize {
    // SAFETY: BYOND's lookup of the list the builder made; its count is at least the creation's 1.
    unsafe {
        let list = LOOKUP.get().get()(id);
        if list != 0 {
            code::write::<u32>(list + REFS, code::read::<u32>(list + REFS) - 1);
        }
        list
    }
}

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut sites = Vec::new();
    for (ida, create, lookup) in BUILDERS {
        let at = core.find_unique(ida, "database list builder")?;
        sites.push((at, core.call_target(at + create)?, at + lookup));
    }
    let (_, create, lookup) = sites[0];
    let lookup = core.call_target(lookup)?;
    for &(_, c, l) in &sites {
        ensure!(c == create && core.call_target(l)? == lookup, "database list builders: different functions");
    }
    LOOKUP.commit(cx, Foreign::at(lookup));
    for &(_, _, l) in &sites {
        core.retarget(l + 1, fresh as Lookup)?;
    }
    Ok(core.report(&[sites[0].0, sites[1].0]))
}
