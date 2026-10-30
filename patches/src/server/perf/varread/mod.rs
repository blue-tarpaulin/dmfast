//! `varread`: reads of user-defined vars by name (`x = obj.var`) answer from a cache of each type's vars (Windows).
//!
//! Stock searches the object's own (changed) vars, then computes its type and searches the type's initial values,
//! then its declarations (a static var reads the statics table; anything else is an "undefined variable" error). The
//! hook searches the own vars the same way and caches the type's answer: the initial value, as BYOND returns it, or a
//! static var's index (its value is read from the statics table each time). Datums, objs, mobs, and areas are read
//! inline with BYOND's own offsets, so a hit computes no type.
//!
//! It's safe because a type's initial values and declarations are built when a .dmb loads and never change after
//! (`deps` guards the read and the id-array intern). The cache is emptied when a .dmb load starts and when the type
//! table moves or changes size. Anything else (a dead object, no type, an undefined var) goes to BYOND's read with the
//! original arguments, so errors come from BYOND. The cache belongs to the first thread to read a var; other threads
//! use BYOND's read. `DMFAST_VERIFY=1` checks every answer against BYOND's read.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod cache;
#[cfg(windows)]
mod verify;
#[cfg(windows)]
mod windows;

pub static VARREAD: Feature =
    Feature::perf(Hosts::SERVER, "varread", "user var read", install!(windows: windows::install)).stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    cache::print_stats();
}
