//! `refskip`: BYOND adds and drops a reference on every Value it copies or discards, through two functions that set
//! up an exception frame before dispatching on the Value's type. For most types the case does nothing (numbers, null,
//! and 32 others) or calls one helper with the Value's data (strings, lists, datums, mobs, ...). Calls to the two
//! functions now go to a stub that looks the type up in a table built from the functions' own dispatch: it returns
//! at once, calls the helper itself, or runs the function (Windows).
//!
//! It's exact: the table is BYOND's dispatch, read case by case, and the frame the stub skips has nothing to unwind.
//! No dependency guard: the table is read from the functions themselves at install.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(windows)]
mod windows;

pub static REFSKIP: Feature =
    Feature::perf(Hosts::SERVER, "refskip", "reference count dispatch", install!(windows: windows::install));
