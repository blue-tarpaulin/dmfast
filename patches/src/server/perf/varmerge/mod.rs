//! `varmerge`: when a .dmb loads, each type's var list is built by inserting its parent's vars, then its own, one at
//! a time into a list kept in name order, each at the place a binary search over the list so far finds. The parent's
//! list is already in that order, so each of its vars lands at the end; here each insert first compares with the
//! last entry and appends when it is greater (Linux).
//!
//! It's exact: the list so far stays sorted (BYOND's search keeps it so), so a key above the last one is above all of
//! them, and the search would end past the last entry and append too. Any other insert runs BYOND's search.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;

pub static VARMERGE: Feature =
    Feature::perf(Hosts::SERVER, "varmerge", "var list merge append", install!(linux: linux::install));
