//! [`Image`]'s searches: IDA-style patterns, bytes, and calls, in its code, its data, or a range.

use super::{Hits, Image, relied};
use crate::scan::{self, Hay};
use crate::{Result, bail};

impl Image {
    /// (address, size) of the executable sections or segments.
    pub fn code_ranges(&self) -> Vec<(usize, usize)> {
        self.parts.iter().filter(|p| p.2).map(|&(a, n, _)| (a, n)).collect()
    }

    /// The parts whose executable flag is `exec` (`None`: all mapped ranges), as shipped.
    pub(crate) fn hays(&self, exec: Option<bool>) -> Result<Vec<Hay<'_>>> {
        self.open()?;
        let ranges: Vec<(usize, usize)> = match exec {
            Some(x) => self.parts.iter().filter(|p| p.2 == x).map(|&(a, n, _)| (a, n)).collect(),
            None => self.mapped.clone(),
        };
        // Mapped (`with_parts`); `Err` only after install.
        ranges.into_iter().map(|(a, n)| Ok((a, self.haystack(a, n)?))).collect()
    }

    /// All matches of an IDA-style pattern (`8B 1D ?? ...`) in the executable parts.
    pub fn find_all(&self, ida: &str) -> Result<Hits> {
        let hays = self.hays(Some(true))?;
        let pat = scan::parse(ida)?;
        Ok(relied(scan::find_pattern(&hays, &pat), pat.len()))
    }

    /// [`Image::find_all`] relying on nothing: candidates for a later check, which relies on what it keeps.
    pub fn probe_all(&self, ida: &str) -> Result<Hits> {
        Ok(Hits(scan::find_pattern(&self.hays(Some(true))?, &scan::parse(ida)?)))
    }

    /// The only match of [`Image::probe_all`] that `keep` accepts, relied on (`Err` naming `what` otherwise). The other
    /// matches aren't relied on, only what `keep` reads in them (it reads as usual: `rel32`, `u32_at`, ... rely).
    pub fn probe_unique(&self, ida: &str, what: &str, keep: impl Fn(usize) -> bool) -> Result<usize> {
        let pat = scan::parse(ida)?;
        let hits = scan::find_pattern(&self.hays(Some(true))?, &pat);
        let at = hits.into_iter().filter(|&a| keep(a)).collect::<Hits>().unique(what)?;
        self.rely(at, pat.len())?;
        Ok(at)
    }

    /// The first match of [`Image::probe_all`] that `keep` accepts, relied on as in [`Image::probe_unique`] (`Err`
    /// naming `what` if none).
    pub fn probe_first(&self, ida: &str, what: &str, keep: impl Fn(usize) -> bool) -> Result<usize> {
        let pat = scan::parse(ida)?;
        let Some(at) = scan::find_pattern(&self.hays(Some(true))?, &pat).into_iter().find(|&a| keep(a)) else {
            bail!("{what}: not found");
        };
        self.rely(at, pat.len())?;
        Ok(at)
    }

    /// The only match of [`Image::find_all`] (`Err` naming `what` otherwise).
    pub fn find_unique(&self, ida: &str, what: &str) -> Result<usize> {
        self.find_all(ida)?.unique(what)
    }

    /// All matches of an IDA-style pattern in `addr..addr+len`.
    pub fn find_in(&self, addr: usize, len: usize, ida: &str) -> Result<Hits> {
        let pat = scan::parse(ida)?;
        Ok(relied(scan::find_pattern(&[(addr, self.haystack(addr, len)?)], &pat), pat.len()))
    }

    /// The only match of [`Image::find_in`] (`Err` naming `what` otherwise).
    pub fn find_unique_in(&self, addr: usize, len: usize, ida: &str, what: &str) -> Result<usize> {
        self.find_in(addr, len, ida)?.unique(what)
    }

    /// Whether an IDA-style pattern matches at `addr`.
    pub fn is_at(&self, addr: usize, ida: &str) -> Result<bool> {
        Ok(self.find_in(addr, ida.split_whitespace().count(), ida)?.as_slice() == [addr])
    }

    /// Every occurrence of `needle` in `addr..addr+len`.
    pub fn find_bytes_in(&self, addr: usize, len: usize, needle: &[u8]) -> Result<Hits> {
        Ok(relied(scan::find_bytes(&[(addr, self.haystack(addr, len)?)], needle), needle.len()))
    }

    /// Whether `addr..addr+len` contains the little-endian dword `v` (an absolute address as it appears in code).
    pub fn mentions(&self, addr: usize, len: usize, v: usize) -> Result<bool> {
        Ok(!self.find_bytes_in(addr, len, &(v as u32).to_le_bytes())?.is_empty())
    }

    /// Every `call rel32` (`E8`) to `target` in the executable parts.
    pub fn calls_to(&self, target: usize) -> Result<Hits> {
        Ok(relied(scan::find_calls(&self.hays(Some(true))?, target), 5))
    }

    /// [`Image::calls_to`] relying on nothing, as [`Image::probe_all`].
    pub fn probe_calls_to(&self, target: usize) -> Result<Hits> {
        Ok(Hits(scan::find_calls(&self.hays(Some(true))?, target)))
    }

    /// Every `call rel32` (`E8`) to `target` in `addr..addr+len`.
    pub fn calls_in(&self, addr: usize, len: usize, target: usize) -> Result<Hits> {
        Ok(relied(scan::find_calls(&[(addr, self.haystack(addr, len)?)], target), 5))
    }

    /// Every occurrence of `needle` in the non-executable parts (a pointer in data, not bytes inside code).
    pub fn find_data(&self, needle: &[u8]) -> Result<Hits> {
        Ok(relied(scan::find_bytes(&self.hays(Some(false))?, needle), needle.len()))
    }

    /// Every occurrence of `needle` anywhere in the image (Windows: headers too).
    pub fn find_bytes(&self, needle: &[u8]) -> Result<Hits> {
        Ok(relied(scan::find_bytes(&self.hays(None)?, needle), needle.len()))
    }

    /// [`Image::find_bytes`] relying on nothing, as [`Image::probe_all`].
    pub fn probe_bytes(&self, needle: &[u8]) -> Result<Hits> {
        Ok(Hits(scan::find_bytes(&self.hays(None)?, needle)))
    }
}
