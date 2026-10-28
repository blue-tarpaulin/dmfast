//! `dirs`: the compiler's directory listings come from a per-directory cache, and entries that can't be directories
//! aren't opened to check (Linux).
//!
//! libbyond resolves each path case-insensitively by listing every directory on the way: it changes into the
//! directory, reads `.`, and opens every entry to see whether it is a directory. A cached listing is replayed while
//! the directory keeps its device, inode, mtime, and ctime; one changed within the last two seconds isn't cached,
//! since timestamps can be coarse. On a cache hit, the change of directory is skipped unless something needs it. An
//! entry whose `d_type` rules out a directory isn't opened; others are opened once per cached listing. Not noticed:
//! a permission change on a subdirectory during a compile. `DMFAST_VERIFY=1` re-reads each hit and compares.
//!
//! `deps` guards the listing function (nothing else in it may use the directory or change an entry while its change
//! of directory is skipped) and `ChangeDir` (it must do no more than change directory, and its cached cwd's users).

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod verify;

pub static DIRS: Feature =
    Feature::perf(Hosts::COMPILER, "dirs", "dir listing", install!(linux: linux::install)).stats(print_stats);

fn print_stats() {
    #[cfg(target_os = "linux")]
    linux::print_stats();
}
