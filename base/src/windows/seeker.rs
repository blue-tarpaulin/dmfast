//! Puts BYOND's bytes back at every DreamSeeker patch site in byondcore.dll and byondwin.dll before features install,
//! so servers and the compiler run stock code and dmfast's patterns match. It writes shipped bytes, so it is outside
//! [`crate::ledger`]. A site that doesn't match the manifest is left alone, with a warning.

use dmfast_seeker::format::{CODE_SECTION, Entry, JMP_LEN, Manifest};

use super::CORE_NAME;
use crate::image::Image;
use crate::{Result, ensure};

/// The DLLs DreamSeeker's patches go into.
const DLLS: [&str; 2] = [CORE_NAME, "byondwin.dll"];

/// Restore every site in the loaded DLLs; under `DMFAST_STATS`, say how many were put back.
///
/// # Safety
/// Loader entry: the DLLs are mapped and relocated, no other thread runs their code.
pub(crate) unsafe fn restore_sites() {
    let mut total = 0;
    for name in DLLS {
        // Not loaded in this program: nothing of it runs here.
        let Ok(image) = Image::loaded(name) else { continue };
        // SAFETY: loader entry (caller).
        match unsafe { restore(name, &image) } {
            Ok(n) => total += n,
            Err(e) => eprintln!("dmfast: {name}: DreamSeeker patch sites not put back - {e}"),
        }
    }
    if total > 0 && crate::driver::stats() {
        eprintln!("dmfast: put BYOND's code back at {total} DreamSeeker patch sites");
    }
}

/// Restore `image`'s sites: how many (0 without DreamSeeker patches).
///
/// # Safety
/// As [`restore_sites`].
unsafe fn restore(name: &str, image: &Image) -> Result<usize> {
    let Some((at, len)) = image.section(&CODE_SECTION)? else { return Ok(0) };
    let manifest = Manifest::parse(image.live_bytes(at, len)?).ok_or("its manifest doesn't parse")?;
    let mut restored = 0;
    for e in &manifest.entries {
        let stock = match original(image, e) {
            Ok(stock) => stock,
            Err(why) => {
                eprintln!("dmfast: {name}: left DreamSeeker patch {} at +{:#x} alone - {why}", e.name, e.site);
                continue;
            }
        };
        // SAFETY: the site is the jump the manifest describes, in a DLL nothing runs yet (caller); `stock` is
        // BYOND's own instructions from there.
        unsafe { crate::poke_code(image.base + e.site as usize, &stock) }?;
        restored += 1;
    }
    Ok(restored)
}

/// The `displace` bytes `e`'s site had in BYOND's DLL, after checking the site and the copy of its original
/// instructions are as the patcher writes them.
fn original(image: &Image, e: &Entry) -> Result<Vec<u8>> {
    ensure!(e.displace >= JMP_LEN, "displaced length {} is shorter than a jump", e.displace);
    let live = image.live_bytes(image.base + e.site as usize, e.displace as usize)?;
    ensure!(live == e.site_bytes(), "the site isn't a jump to its gate");
    let copy = image.live_bytes(image.base + e.orig as usize, (e.displace + JMP_LEN) as usize)?;
    let (stock, back) = copy.split_at(e.displace as usize);
    ensure!(back == e.jump_back(), "its original instructions don't jump back");
    Ok(stock.to_vec())
}
