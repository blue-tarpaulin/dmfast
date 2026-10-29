//! [`Image`]: a loaded module (the core library, the program) as install code sees it, from
//! [`Ctx`](crate::feature::Ctx). Its readers and searches are bounds-checked (`Err` outside its mapped ranges), read
//! code as shipped while features install (`crate::snapshot`) and record what they return as relied on
//! ([`crate::ledger`]); [`Image::haystack`] and the probes (`probe_*`) don't, and [`Image::rely`] relies on a range
//! explicitly. Its writers only record the installing feature's writes (`Err` outside an install); the ledger does
//! them. Searches return [`Hits`] (`Err` after install, or outside the image for an address range).
//!
//! Images exist only during install; one used later reads and writes nothing and is reported as a bug.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use crate::code::{CodeAddr, branch};
use crate::ledger::{self, rely};
use crate::scan::{self, Hay};
use crate::{Result, bail};

/// Install is over ([`close`]).
static CLOSED: AtomicBool = AtomicBool::new(false);
/// An image was used after [`close`]: a bug, reported at exit.
static USED_LATE: AtomicBool = AtomicBool::new(false);

/// End install: images read, find and write nothing from here on (reads and writes are `Err`).
pub(crate) fn close() {
    CLOSED.store(true, Relaxed);
}

pub(crate) fn used_late() -> bool {
    USED_LATE.load(Relaxed)
}

/// A loaded module: its mapped ranges and its sections (Windows) or segments (Linux).
pub struct Image {
    /// Load address: the module handle (Windows), the load bias (Linux).
    pub base: usize,
    /// Readable ranges (address, size): the whole image (Windows), each segment (Linux).
    mapped: Vec<(usize, usize)>,
    /// Sections or segments (address, size, executable), each inside `mapped`.
    parts: Vec<(usize, usize, bool)>,
    /// The dynamic section.
    #[cfg(target_os = "linux")]
    pub(crate) dynamic: Option<usize>,
    /// The object's path for `dlopen` (`None`: the main program).
    #[cfg(target_os = "linux")]
    pub(crate) path: Option<std::ffi::CString>,
    _install_only: PhantomData<*const ()>,
}

impl Image {
    /// An image of `mapped` without parts (see [`Image::with_parts`]).
    ///
    /// # Safety
    /// Every range in `mapped` stays mapped and readable while the image exists (install time).
    pub(crate) unsafe fn new(base: usize, mapped: Vec<(usize, usize)>) -> Image {
        Image {
            base,
            mapped,
            parts: Vec::new(),
            #[cfg(target_os = "linux")]
            dynamic: None,
            #[cfg(target_os = "linux")]
            path: None,
            _install_only: PhantomData,
        }
    }

    /// The image with its sections or segments (address, size, executable); `Err` if one isn't mapped.
    pub(crate) fn with_parts(mut self, parts: Vec<(usize, usize, bool)>) -> Result<Image> {
        if let Some(&(a, n, _)) = parts.iter().find(|&&(a, n, _)| !self.is_mapped(a, n)) {
            bail!("{a:#x}+{n:#x} is not mapped");
        }
        self.parts = parts;
        Ok(self)
    }

    pub(crate) fn is_mapped(&self, addr: usize, len: usize) -> bool {
        within(self.mapped.iter().copied(), addr, len)
    }

    /// `Err` once install is over ([`close`]), recorded as a bug.
    pub(crate) fn open(&self) -> Result<()> {
        if CLOSED.load(Relaxed) {
            USED_LATE.store(true, Relaxed);
            return Err("image used after install".into());
        }
        Ok(())
    }

    fn check(&self, addr: usize, len: usize) -> Result<()> {
        self.open()?;
        if self.is_mapped(addr, len) { Ok(()) } else { Err(format!("{addr:#x}+{len:#x} is outside the image").into()) }
    }

    /// The dword at `addr` as it is now, with earlier features' hooks, not relied on.
    pub fn live_u32(&self, addr: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(*self.live_bytes(addr, 4)?.first_chunk().ok_or("short read")?))
    }

    /// `addr..addr+len` as it is now, with earlier features' hooks, not relied on: headers, tables, hooks.
    pub fn live_bytes(&self, addr: usize, len: usize) -> Result<&[u8]> {
        self.check(addr, len)?;
        // SAFETY: mapped (`new`); writes wait for the ledger's commit, after install code drops its borrows.
        Ok(unsafe { std::slice::from_raw_parts(addr as *const u8, len) })
    }

    /// The NUL-terminated string at `addr`, without the NUL, as it is now.
    pub(crate) fn c_str(&self, addr: usize) -> Result<&[u8]> {
        let &(start, len) = self
            .mapped
            .iter()
            .find(|&&(s, n)| (s..s + n).contains(&addr))
            .ok_or_else(|| format!("{addr:#x} is outside the image"))?;
        let rest = self.live_bytes(addr, start + len - addr)?;
        let end = memchr::memchr(0, rest).ok_or_else(|| format!("no string end after {addr:#x}"))?;
        Ok(&rest[..end])
    }

    /// `addr..addr+len` as shipped, not relied on: something to search through.
    pub fn haystack(&self, addr: usize, len: usize) -> Result<&[u8]> {
        self.check(addr, len)?;
        match crate::snapshot::at(addr, len) {
            // SAFETY: the copy outlives every image (`crate::driver::run` drops them before releasing it).
            Some(p) => Ok(unsafe { std::slice::from_raw_parts(p, len) }),
            None => self.live_bytes(addr, len),
        }
    }

    /// `addr..addr+len` as shipped, relied on.
    pub fn bytes(&self, addr: usize, len: usize) -> Result<&[u8]> {
        let b = self.haystack(addr, len)?;
        rely(addr, len);
        Ok(b)
    }

    /// The `N` bytes at `addr` as shipped, relied on.
    pub fn array<const N: usize>(&self, addr: usize) -> Result<[u8; N]> {
        self.bytes(addr, N)?.first_chunk().copied().ok_or_else(|| "short read".into())
    }

    /// The dword at `addr` as shipped, relied on.
    pub fn u32_at(&self, addr: usize) -> Result<u32> {
        self.array(addr).map(u32::from_le_bytes)
    }

    /// The absolute address operand at `addr`, as shipped, relied on.
    pub fn ptr_at(&self, addr: usize) -> Result<usize> {
        self.u32_at(addr).map(|v| v as usize)
    }

    /// Target of the rel32 field at `field` (the instruction ends right after it).
    pub fn rel32(&self, field: usize) -> Result<usize> {
        Ok(field.wrapping_add(4).wrapping_add(self.u32_at(field)? as i32 as isize as usize))
    }

    /// `Err(err)` unless `addr` holds `bytes` as shipped (relied on).
    pub fn expect(&self, addr: usize, bytes: &[u8], err: &'static str) -> Result<()> {
        if self.bytes(addr, bytes.len())? != bytes {
            return Err(err.into());
        }
        Ok(())
    }

    /// The installing feature relies on `addr..addr+len` (a match a probe found, a range read with
    /// [`Image::haystack`]).
    pub fn rely(&self, addr: usize, len: usize) -> Result<()> {
        self.check(addr, len)?;
        rely(addr, len);
        Ok(())
    }

    /// Target of the rel8 field at `field` (the instruction ends right after it).
    pub fn rel8(&self, field: usize) -> Result<usize> {
        let [b] = self.array(field)?;
        Ok((field + 1).wrapping_add(b as i8 as isize as usize))
    }

    /// (address, size) of the executable sections or segments.
    pub fn code_ranges(&self) -> Vec<(usize, usize)> {
        self.parts.iter().filter(|p| p.2).map(|&(a, n, _)| (a, n)).collect()
    }

    /// Fails if a relocation, or a direct branch (`E8`, `E9`, or `0F 8x`) from elsewhere in the code, lands inside one
    /// of `ranges` (`(at, len)`), code about to be rewritten in place. A branch to a range's start is fine, and so is
    /// one from inside a range, which is rewritten too.
    pub fn untouched(&self, ranges: &[(usize, usize)]) -> Result<()> {
        let hits = |lo: usize, hi: usize| ranges.iter().any(|&(at, len)| hi > at && lo < at + len);
        if let Some(r) = self.reloc_sites().into_iter().find(|&r| hits(r, r + 4)) {
            bail!("relocation at {r:#x} in rewritten code");
        }
        for (start, n) in self.code_ranges() {
            let bytes = self.haystack(start, n)?;
            for i in 0..n.saturating_sub(6) {
                let op = match bytes[i] {
                    0xE8 | 0xE9 => 1,
                    0x0F if bytes[i + 1] & 0xF0 == 0x80 => 2,
                    _ => continue,
                };
                let from = start + i;
                let rel = i32::from_le_bytes([bytes[i + op], bytes[i + op + 1], bytes[i + op + 2], bytes[i + op + 3]]);
                let to = (from + op + 4).wrapping_add(rel as isize as usize);
                // `hits(to, to)`: strictly inside a range.
                if hits(to, to) && !hits(from, from + 1) {
                    bail!("branch at {from:#x} into rewritten code at {to:#x}");
                }
            }
        }
        Ok(())
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

    /// Overwrite code bytes (in the executable parts) when the installing feature succeeds.
    pub fn write_code(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(true, addr, data, false)
    }

    /// [`Image::write_code`] of `jmp target` at `site`, NOP-padded to `len` (>= 5).
    pub fn jump(&self, site: usize, len: usize, target: impl CodeAddr) -> Result<()> {
        self.write_code(site, &branch(0xE9, site, target.code_addr(), len))
    }

    /// [`Image::write_code`] of `call target` at `site`, NOP-padded to `len` (>= 5).
    pub fn call(&self, site: usize, len: usize, target: impl CodeAddr) -> Result<()> {
        self.write_code(site, &branch(0xE8, site, target.code_addr(), len))
    }

    /// [`Image::write_code`] over another feature's hook that the new code calls through (it is in this one's
    /// `after` list).
    pub fn wrap_code(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(true, addr, data, true)
    }

    /// Overwrite data bytes (in the non-executable parts: a pointer table entry, an import slot) when the installing
    /// feature succeeds. The page keeps its permissions.
    pub fn write_data(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(false, addr, data, false)
    }

    /// [`Image::write_data`] over another feature's write, as [`Image::wrap_code`].
    pub fn wrap_data(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(false, addr, data, true)
    }

    /// A write inside one executable (`exec`) or non-executable part.
    fn record(&self, exec: bool, addr: usize, data: &[u8], wrap: bool) -> Result<()> {
        if !self.in_parts(exec, addr, data.len()) {
            let what = if exec { "code" } else { "data" };
            bail!("write at {addr:#x}+{:#x}: outside the image's {what}", data.len());
        }
        let poke: ledger::Poke = if exec { crate::poke_code } else { crate::poke_data };
        ledger::record(addr, data, poke, wrap)
    }

    /// `addr..addr+len` lies in one executable (`exec`) or non-executable part.
    fn in_parts(&self, exec: bool, addr: usize, len: usize) -> bool {
        within(self.parts.iter().filter(|p| p.2 == exec).map(|&(a, n, _)| (a, n)), addr, len)
    }
}

/// `addr..addr+len` lies inside one of `ranges` (address, size).
fn within(ranges: impl IntoIterator<Item = (usize, usize)>, addr: usize, len: usize) -> bool {
    ranges.into_iter().any(|(s, n)| s <= addr && addr.checked_add(len).is_some_and(|e| e <= s + n))
}

/// The little-endian dword at `at` in `b` (a header or table the OS parts parse).
pub(crate) fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*b.get(at..)?.first_chunk()?))
}

/// `hits`, each relied on as a match `len` bytes long.
fn relied(hits: Vec<usize>, len: usize) -> Hits {
    for &a in &hits {
        rely(a, len);
    }
    Hits(hits)
}

/// A search's matches: addresses in image order (they may overlap).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hits(Vec<usize>);

impl Hits {
    /// The only match; `Err("{what}: expected 1 match, found {n}")` otherwise.
    pub fn unique(self, what: &str) -> Result<usize> {
        match self.0[..] {
            [a] => Ok(a),
            _ => bail!("{what}: expected 1 match, found {}", self.0.len()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The first match.
    pub fn first(&self) -> Option<usize> {
        self.0.first().copied()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, usize> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[usize] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<usize> {
        self.0
    }
}

impl IntoIterator for Hits {
    type Item = usize;
    type IntoIter = std::vec::IntoIter<usize>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Hits {
    type Item = &'a usize;
    type IntoIter = std::slice::Iter<'a, usize>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Matches a feature found itself (e.g. by walking relocations), in its order.
impl FromIterator<usize> for Hits {
    fn from_iter<I: IntoIterator<Item = usize>>(iter: I) -> Hits {
        Hits(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Hits, Image};
    use crate::ledger::{begin, discard, tests::relied_now};

    /// Reads and searches stay inside the mapped ranges; writes need an install.
    #[test]
    fn reads_stay_inside_the_image() {
        let _serial = crate::sync::lock(&crate::ledger::tests::SERIAL);
        let code = [0x90u8, 0xE8, 1, 0, 0, 0, 0xC3, 0xEB];
        let a = code.as_ptr() as usize;
        // SAFETY: `code` outlives the images.
        let image = |parts| unsafe { Image::new(a, vec![(a, 8)]) }.with_parts(parts);
        assert!(image(vec![(a, 9, true)]).is_err());
        let image = image(vec![(a, 8, true)]).unwrap();
        assert_eq!((image.u32_at(a + 2), image.rel32(a + 2), image.rel8(a + 7)), (Ok(1), Ok(a + 7), Ok(a - 13)));
        let (good, bad) = (image.expect(a + 6, &[0xC3, 0xEB], "x"), image.expect(a + 1, &[0xE9], "x"));
        assert_eq!((image.ptr_at(a + 2), good, bad), (Ok(1), Ok(()), Err("x".into())));
        assert!(image.u32_at(a + 5).is_err() && image.bytes(a - 1, 1).is_err() && image.bytes(usize::MAX, 2).is_err());
        let (all, calls) = (image.find_all("E8 ?? 00 00 00"), image.calls_to(a + 7));
        assert_eq!((all, calls), (Ok(Hits(vec![a + 1])), Ok(Hits(vec![a + 1]))));
        assert!(image.find_in(a + 4, 8, "C3").is_err() && image.write_code(a, &[0xCC]).is_err());
    }

    /// Searches rely on each match (absolute addresses), probes on nothing; `rely` only inside the image.
    #[test]
    fn searches_rely_on_their_matches_and_probes_on_nothing() {
        let _serial = crate::sync::lock(&crate::ledger::tests::SERIAL);
        let code = [0x90u8, 0xE8, 1, 0, 0, 0, 0xC3, 0xEB, 0x90, 0xC3];
        let a = code.as_ptr() as usize;
        // SAFETY: `code` outlives the image.
        let image = unsafe { Image::new(a, vec![(a, 10)]) }.with_parts(vec![(a, 10, true)]).unwrap();
        begin();
        let probes = (image.probe_all("90 ?? ?? ?? ?? ?? C3"), image.probe_bytes(&[0xC3]), image.probe_calls_to(a + 7));
        assert_eq!(probes, (Ok(Hits(vec![a])), Ok(Hits(vec![a + 6, a + 9])), Ok(Hits(vec![a + 1]))));
        assert!(relied_now().is_empty());
        assert_eq!(image.find_bytes_in(a + 2, 8, &[0xC3]), Ok(Hits(vec![a + 6, a + 9])));
        assert_eq!(image.find_all("90 ?? ?? ?? ?? ?? C3").and_then(|h| h.unique("x")), Ok(a));
        assert!(image.rely(a + 8, 3).is_err());
        assert_eq!(image.rely(a + 8, 2), Ok(()));
        assert_eq!(relied_now(), [(a + 6, a + 7), (a + 9, a + 10), (a, a + 7), (a + 8, a + 10)]);
        discard();
        assert_eq!(Hits(vec![a, a]).unique("x"), Err("x: expected 1 match, found 2".into()));
        assert_eq!(Hits::default().unique("x"), Err("x: expected 1 match, found 0".into()));
    }
}
