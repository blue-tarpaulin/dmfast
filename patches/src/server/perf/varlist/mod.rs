//! `varlist`: a type with no vars of its own reuses its parent's var list instead of merging it again (Windows).
//!
//! When a .dmb loads, each type's var list is its own (name, value) pairs merged over its parent's list, then
//! interned. With no own pairs the merge copies the parent's list, which is already sorted and unique, so the intern
//! finds the parent's list itself. The hook returns that list's id and takes the reference the merge would have.
//!
//! It's safe because the hook only answers for a parent list the merge returned during the current load, and only
//! while the list table still holds that list with the same data and count (`deps` guards the merge and the load's
//! var list pass). `DMFAST_VERIFY=1` runs the merge anyway and compares ids.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod verify;
#[cfg(windows)]
mod windows;

pub static VARLIST: Feature =
    Feature::perf(Hosts::SERVER, "varlist", "var list merge", install!(windows: windows::install)).stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    windows::print_stats();
}
