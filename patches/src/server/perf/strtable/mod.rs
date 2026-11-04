//! `strtable`: the .dmb loader's pass over the string table as one native loop, without the rebuild BYOND runs
//! after it.
//!
//! BYOND's pass sets each entry's id, clears its links and reference count, looks up its string (linking the entry
//! when new), fills in its length, and sets a CRC over id and string. [`strtree`]'s `load_all` does the same with its
//! index and [`crc`]'s CRC. The rebuild then re-checks those CRCs (always equal), zeroes the links again, and with
//! `strtree` installed stops without relinking; in `strtree`'s fast mode nothing reads the links, so it has nothing
//! left to do. Not in `strtree`'s verify mode (`DMFAST_VERIFY=1`), where the real tree stays linked.
//!
//! Dependency guards (`deps`): the pass, and what `strtree` relies on.
//!
//! [`strtree`]: crate::server::perf::strtree
//! [`crc`]: crate::server::perf::crc

use crate::base::feature::{Feature, Hosts, install};
use crate::server::perf::crc::CRC;
use crate::server::perf::strtree::STRTREE;

#[cfg(windows)]
mod verify;
#[cfg(windows)]
mod windows;

pub static STRTABLE: Feature =
    Feature::perf(Hosts::SERVER, "strtable", "string table load", install!(windows: windows::install))
        .requires(&[&STRTREE, &CRC])
        .stats(print_stats);

fn print_stats() {
    #[cfg(windows)]
    verify::print_stats();
}
