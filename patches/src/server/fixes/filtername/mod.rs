//! `filtername` (forum report 2992239): a named filter added to `filters` replaces the filter of that name.
//!
//! An appearance has at most one filter of a given name. Assigning a list (`filters = list(...)`) keeps to that: a
//! filter whose name is already in the list removes the earlier one, and goes at the end. Adding one to an atom's or
//! image's `filters` (`+=`, `Add()`, `Insert()`, `filters[i] = f`) didn't, so both stayed. The fix makes the add drop
//! any other filter of that name the same way; the new filter goes where the add puts it. Unnamed filters are never
//! dropped. Only an add whose filter's name is already there changes, and its new chain comes from BYOND's own splice.

use std::slice;

use crate::base::code::{self, Foreign, live_u32};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static FILTERNAME: Feature = Feature::fix(
    Hosts::SERVER,
    "filtername",
    "a named filter added to filters replaces the one of that name",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// Appends a filter id to a filter chain: the new chain.
type Append = unsafe extern "C" fn(u32, u32) -> u32;
/// Replaces `count` filters of a chain from `at` with `n` ids: the new chain.
type Splice = unsafe extern "C" fn(u32, u32, u32, *const u32, u32) -> u32;

/// A chain's filter ids offset (its count is the `u16` at 0).
const IDS: usize = 8;
/// A filter's name offset: a string id, 0 or 0xFFFF for none.
const NAME: usize = 0x30;

/// Where the filter chains and filters are: the chain count and array globals, the global holding the filter table,
/// and that table's count and array offsets in it.
struct Tables {
    chain_count: usize,
    chain_array: usize,
    state: usize,
    filter_count: usize,
    filter_array: usize,
}

impl Tables {
    /// The filter ids of `chain`.
    ///
    /// # Safety
    /// BYOND's tables are live and nothing changes them while the slice is used.
    unsafe fn chain(&self, chain: u32) -> &[u32] {
        // SAFETY: the chain table and an existing chain (caller).
        unsafe {
            if chain >= live_u32(self.chain_count) {
                return &[];
            }
            let c = live_u32(live_u32(self.chain_array) as usize + 4 * chain as usize) as usize;
            if c == 0 {
                return &[];
            }
            let n = usize::from(code::read::<u16>(c));
            if n == 0 {
                return &[];
            }
            slice::from_raw_parts(live_u32(c + IDS) as *const u32, n)
        }
    }

    /// The name of filter `id`, if it has one.
    ///
    /// # Safety
    /// BYOND's tables are live.
    unsafe fn name(&self, id: u32) -> Option<u32> {
        // SAFETY: the filter table and an existing filter (caller).
        unsafe {
            let state = live_u32(self.state) as usize;
            if id >= live_u32(state + self.filter_count) {
                return None;
            }
            let f = live_u32(live_u32(state + self.filter_array) as usize + 4 * id as usize) as usize;
            if f == 0 {
                return None;
            }
            let name = live_u32(f + NAME);
            (name != 0 && name != 0xFFFF).then_some(name)
        }
    }
}

/// What the add's two calls need: the tables, and the append and splice they called.
struct Found {
    tables: Tables,
    append: Foreign<Append>,
    splice: Foreign<Splice>,
}

static FOUND: Installed<Found> = Installed::new();

/// The add's append (`filters += f` and the like): drops the filters of `id`'s name first.
///
/// # Safety
/// BYOND's call: `chain` is a filter chain and `id` a filter.
unsafe extern "C" fn append(chain: u32, id: u32) -> u32 {
    let f = FOUND.get();
    // SAFETY: BYOND's chain and filter (caller).
    unsafe { added(f, chain, None, id).unwrap_or_else(|| f.append.get()(chain, id)) }
}

/// The add's splice, which replaces one filter (`filters[i] = f`): drops the other filters of the new one's name.
///
/// # Safety
/// BYOND's call: `chain` is a filter chain and `ids` holds `n` filters.
unsafe extern "C" fn splice(chain: u32, at: u32, count: u32, ids: *const u32, n: u32) -> u32 {
    let f = FOUND.get();
    // SAFETY: BYOND's chain and filters (caller).
    unsafe {
        if count == 1
            && n == 1
            && let Some(new) = added(f, chain, Some((at & 0xFFFF) as usize), *ids)
        {
            return new;
        }
        f.splice.get()(chain, at, count, ids, n)
    }
}

/// `chain` with `id` put at `at` (over the filter there) or appended (also when `at` is past the end, as the splice
/// does), without the other filters of `id`'s name; `None` if `id` has no name or no other filter has it.
///
/// # Safety
/// `chain` is a filter chain and `id` a filter.
unsafe fn added(f: &Found, chain: u32, at: Option<usize>, id: u32) -> Option<u32> {
    let t = &f.tables;
    // SAFETY: BYOND's chain and filters (caller); the splice only reads `old` and `new`.
    unsafe {
        let name = t.name(id)?;
        let old = t.chain(chain);
        let at = at.filter(|&i| i < old.len());
        let other = |(i, &x): (usize, &u32)| Some(i) != at && t.name(x) == Some(name);
        if !old.iter().enumerate().any(other) {
            return None;
        }
        let mut new = Vec::with_capacity(old.len() + 1);
        for (i, &x) in old.iter().enumerate() {
            if Some(i) == at {
                new.push(id);
            } else if t.name(x) != Some(name) {
                new.push(x);
            }
        }
        if at.is_none() {
            new.push(id);
        }
        Some(f.splice.get()(chain, 0, old.len() as u32, new.as_ptr(), new.len() as u32))
    }
}
