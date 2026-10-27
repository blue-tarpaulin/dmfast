//! Pristine copies of code ranges, read by images during install so every feature sees stock code.

use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use crate::image::Image;

/// One image's copy, and the ones taken before it.
struct Snapshot {
    /// (address, copy) of each code range.
    ranges: Vec<(usize, Box<[u8]>)>,
    older: *mut Snapshot,
}

/// The newest copy; null outside install. Images aren't `Send`: only the install thread takes and reads copies.
static SNAPSHOT: AtomicPtr<Snapshot> = AtomicPtr::new(std::ptr::null_mut());

/// Copy `image`'s code ranges that no copy overlaps yet (an image opened again keeps its first copy).
pub(crate) fn take(image: &Image) {
    let ranges: Vec<(usize, Box<[u8]>)> = image
        .code_ranges()
        .into_iter()
        .filter(|&(a, n)| !copies().any(|(s, c)| a < s + c.len() && s < a + n))
        .filter_map(|(a, n)| Some((a, image.live_bytes(a, n).ok()?.into())))
        .collect();
    if !ranges.is_empty() {
        let new = Box::new(Snapshot { ranges, older: SNAPSHOT.load(Relaxed) });
        SNAPSHOT.store(Box::into_raw(new), Relaxed);
    }
}

/// Drop every copy: the readers read live memory from here on.
/// # Safety
/// No slice returned by [`at`] is still in use.
pub(crate) unsafe fn release() {
    let mut s = SNAPSHOT.swap(std::ptr::null_mut(), Relaxed);
    while !s.is_null() {
        // SAFETY: made by `take`, no longer shared (caller).
        s = unsafe { Box::from_raw(s) }.older;
    }
}

/// Where `addr..addr + len` is in the copy, if it lies wholly inside one copied range.
pub(crate) fn at(addr: usize, len: usize) -> Option<*const u8> {
    copies().find_map(|(start, copy)| {
        let off = addr.checked_sub(start)?;
        (off.checked_add(len)? <= copy.len()).then(|| copy[off..].as_ptr())
    })
}

/// Every copied range (address, copy), newest image first; valid until [`release`].
fn copies() -> impl Iterator<Item = (usize, &'static [u8])> {
    // SAFETY: copies live until `release`, whose caller ensures no reader runs.
    let node = |p: *mut Snapshot| unsafe { p.as_ref() };
    std::iter::successors(node(SNAPSHOT.load(Relaxed)), move |s| node(s.older))
        .flat_map(|s| s.ranges.iter().map(|(a, c)| (*a, &c[..])))
}

#[cfg(test)]
mod tests {
    use crate::image::Image;

    /// While the copies exist, image readers see the code as first copied; `live_bytes` sees it as it is.
    #[test]
    fn readers_see_the_copy() {
        let _serial = crate::sync::lock(&crate::ledger::tests::SERIAL);
        let (mut code, mut later) = ([0x90u8, 0xC3], [0x55u8]);
        let (a, b) = (code.as_mut_ptr() as usize, later.as_mut_ptr() as usize);
        // SAFETY: `code` and `later` outlive the images.
        let image = |at, n| unsafe { Image::new(at, vec![(at, n)]) }.with_parts(vec![(at, n, true)]).unwrap();
        let (first, second) = (image(a, 2), image(b, 1));
        super::take(&first);
        // SAFETY: no slice of `code` is in use.
        unsafe { (a as *mut u8).write_volatile(0xCC) };
        // Opened again after a write: the first copy stays. Another image is copied when opened.
        super::take(&first);
        super::take(&second);
        // SAFETY: no slice of `later` is in use.
        unsafe { (b as *mut u8).write_volatile(0xCC) };
        assert_eq!((first.bytes(a, 2), first.live_bytes(a, 2)), (Ok(&[0x90, 0xC3][..]), Ok(&[0xCC, 0xC3][..])));
        assert_eq!((second.bytes(b, 1), second.live_bytes(b, 1)), (Ok(&[0x55][..]), Ok(&[0xCC][..])));
        drop((first, second));
        // SAFETY: no slice of the copy is in use.
        unsafe { super::release() };
    }
}
