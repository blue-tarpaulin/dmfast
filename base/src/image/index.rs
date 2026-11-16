//! Install-time indexes for [`Image::untouched`] and the call searches, built once per image instead of rescanning
//! the code on every check: its direct branches and the relocated words in its code, by target.
//!
//! Both hold the code as shipped, so they are used only while code reads give shipped bytes: before any write is
//! applied, or once a snapshot covers the code (install reads code from it). Relocated words in data are read as they
//! are now, on every check.

use super::Image;
use crate::Result;

/// Direct rel32 branches as shipped, as [`edge`]s, sorted.
pub(super) struct Branches {
    pub(super) all: Vec<u64>,
    pub(super) calls: Vec<u64>,
}

/// Relocated words: every site, sorted; the ones in code as [`edge`]s, sorted; the ones in data.
pub(super) struct Relocs {
    pub(super) sites: Vec<usize>,
    pub(super) code: Vec<u64>,
    pub(super) data: Vec<usize>,
}

/// A reference from `site` to `target` (32-bit addresses).
fn edge(target: usize, site: usize) -> u64 {
    (target as u64) << 32 | site as u64
}

/// `v` stably sorted by target: two counting passes of 16 bits (no 64-bit compares, which cost on i686), so equal
/// targets keep their sites in the order they were added.
fn sort_by_target(v: &mut Vec<u64>) {
    let mut tmp = vec![0; v.len()];
    for shift in [32, 48] {
        let mut count = vec![0usize; 1 << 16];
        for &e in v.iter() {
            count[(e >> shift) as usize & 0xFFFF] += 1;
        }
        let mut at = 0;
        for c in &mut count {
            (at, *c) = (at + *c, at);
        }
        for &e in v.iter() {
            let k = (e >> shift) as usize & 0xFFFF;
            tmp[count[k]] = e;
            count[k] += 1;
        }
        std::mem::swap(v, &mut tmp);
    }
}

/// An [`edge`]'s (target, site).
pub(super) fn ends(e: u64) -> (usize, usize) {
    ((e >> 32) as usize, e as u32 as usize)
}

/// The [`edge`]s of `v` (sorted) with `lo < target < hi`.
pub(super) fn strictly_between(v: &[u64], lo: usize, hi: usize) -> &[u64] {
    let a = v.partition_point(|&e| ends(e).0 <= lo);
    let b = v.partition_point(|&e| ends(e).0 < hi);
    &v[a..b.max(a)]
}

impl Image {
    /// Code reads give the code as shipped: nothing written yet, or a snapshot covers every executable part.
    pub(super) fn reads_shipped_code(&self) -> bool {
        crate::ledger::nothing_applied() || self.code_ranges().iter().all(|&(a, n)| crate::snapshot::at(a, n).is_some())
    }

    /// The branch index, unless code reads no longer give shipped bytes (then callers scan).
    pub(super) fn branches(&self) -> Result<Option<&Branches>> {
        self.open()?;
        if !self.reads_shipped_code() {
            return Ok(None);
        }
        if self.branches.get().is_none() {
            let code = self.hays(Some(true))?;
            let (mut all, mut calls) = (Vec::new(), Vec::new());
            for (site, to, call) in crate::scan::branches(&code) {
                all.push(edge(to, site));
                if call {
                    calls.push(edge(to, site));
                }
            }
            sort_by_target(&mut all);
            sort_by_target(&mut calls);
            let _ = self.branches.set(Branches { all, calls });
        }
        Ok(self.branches.get())
    }

    /// The relocation index, unless code reads no longer give shipped bytes (then callers read every word).
    pub(super) fn reloc_index(&self) -> Result<Option<&Relocs>> {
        self.open()?;
        if !self.reads_shipped_code() {
            return Ok(None);
        }
        if self.relocs.get().is_none() {
            let mut sites = self.reloc_sites();
            sites.sort_unstable();
            let code = self.hays(Some(true))?;
            let in_code = |r: usize| code.iter().find(|&&(s, b)| s <= r && r + 4 <= s + b.len());
            let (mut words, mut data) = (Vec::new(), Vec::new());
            for &r in &sites {
                match in_code(r) {
                    Some(&(s, b)) => words.push(edge(super::le32(b, r - s).ok_or("short read")? as usize, r)),
                    None => data.push(r),
                }
            }
            sort_by_target(&mut words);
            let _ = self.relocs.set(Relocs { sites, code: words, data });
        }
        Ok(self.relocs.get())
    }

    /// The relocated word at `r` as install reads it (code as shipped, data as it is now).
    pub(super) fn word(&self, r: usize) -> Result<usize> {
        Ok(u32::from_le_bytes(*self.haystack(r, 4)?.first_chunk().ok_or("short read")?) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::{edge, ends, sort_by_target, strictly_between};

    #[test]
    fn edges_sort_by_target_and_keep_site_order() {
        let mut v: Vec<u64> = [(0x2_0000, 5), (0x1_0005, 1), (0x2_0000, 2), (0xFFFF, 9), (0x1_0005, 0)]
            .iter()
            .map(|&(t, s)| edge(t, s))
            .collect();
        sort_by_target(&mut v);
        let got: Vec<(usize, usize)> = v.iter().map(|&e| ends(e)).collect();
        assert_eq!(got, [(0xFFFF, 9), (0x1_0005, 1), (0x1_0005, 0), (0x2_0000, 5), (0x2_0000, 2)]);
        assert_eq!(strictly_between(&v, 0xFFFF, 0x2_0000).len(), 2);
    }
}
