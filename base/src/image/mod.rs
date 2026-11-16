//! [`Image`]: a loaded module (the core library, the program) as install code sees it, from
//! [`Ctx`](crate::feature::Ctx). Its readers and searches are bounds-checked (`Err` outside its mapped ranges), read
//! code as shipped while features install (`crate::snapshot`; data as it is now) and record what they return as
//! relied on ([`crate::ledger`]); [`Image::haystack`], `live_*`, [`Image::c_str`], and the probes (`probe_*`) don't,
//! and [`Image::rely`] relies on a range explicitly. Its writers only record the installing feature's writes (`Err`
//! outside an install); the ledger does them. Searches return [`Hits`] (`Err` after install, or outside the image for
//! an address range).
//!
//! Images exist only during install; one used later reads and writes nothing and is reported as a bug.

mod find;
mod read;
mod write;

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use crate::code::Cond;
use crate::ledger::rely;
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

    /// The first match; `Err("{what}: not found")` if none.
    pub fn first_or(&self, what: &str) -> Result<usize> {
        self.first().ok_or_else(|| format!("{what}: not found").into())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, usize> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[usize] {
        &self.0
    }
}

/// A jump read from code ([`Image::branch_at`]): where it goes, the instruction after it, and its condition (`None`:
/// a `jmp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Branch {
    pub target: usize,
    pub next: usize,
    pub cond: Option<Cond>,
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
