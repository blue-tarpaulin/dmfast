//! Writing to a loaded object, code and data ([`crate::ledger`] does [`crate::image::Image`]'s recorded writes with
//! [`poke_code`] / [`poke_data`]), and the executable memory behind [`crate::code::exec_code`].

use std::ffi::c_void;

use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_EXEC, PROT_READ, PROT_WRITE, mmap, mprotect, munmap};

use crate::linux::PAGE;
use crate::{Result, bail};

/// `len` fresh writable bytes, to fill and [`seal_exec`].
pub(crate) fn alloc_exec(len: usize) -> Result<usize> {
    // SAFETY: a fresh anonymous mapping; nothing else uses it.
    let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) };
    if p == MAP_FAILED { Err("mmap failed".into()) } else { Ok(p as usize) }
}

/// Make an [`alloc_exec`] block executable (no longer writable).
///
/// # Safety
/// `p..p+len` is one [`alloc_exec`] block.
pub(crate) unsafe fn seal_exec(p: usize, len: usize) -> Result<()> {
    if unsafe { mprotect(p as *mut c_void, len, PROT_READ | PROT_EXEC) } != 0 {
        return Err("mprotect failed".into());
    }
    Ok(())
}

/// Free an [`alloc_exec`] block.
///
/// # Safety
/// `p..p+len` is one [`alloc_exec`] block that nothing runs or points into.
pub(crate) unsafe fn free_exec(p: usize, len: usize) {
    unsafe { munmap(p as *mut c_void, len) };
}

/// Write `data` at `addr` in executable memory.
///
/// # Safety
/// No thread may be executing or reading `addr..addr+data.len()`; the new bytes must be valid code.
pub(crate) unsafe fn poke_code(addr: usize, data: &[u8]) -> Result<()> {
    let start = addr & !(PAGE - 1);
    let len = addr + data.len() - start;
    // SAFETY: `start..start+len` covers loaded code nothing runs (caller); its protection is restored after.
    unsafe {
        if mprotect(start as *mut c_void, len, PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
            bail!("mprotect failed at {addr:#x}");
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
        mprotect(start as *mut c_void, len, PROT_READ | PROT_EXEC);
    }
    Ok(())
}

/// Write `data` at `addr` in one non-executable mapping, keeping its permissions.
///
/// # Safety
/// Nothing may be using `addr..addr+data.len()` while it is written.
pub(crate) unsafe fn poke_data(addr: usize, data: &[u8]) -> Result<()> {
    let maps = std::fs::read_to_string("/proc/self/maps").map_err(|e| format!("/proc/self/maps: {e}"))?;
    let (end, perms) = maps
        .lines()
        .find_map(|l| {
            let (range, rest) = l.split_once(' ')?;
            let (a, b) = range.split_once('-')?;
            let (a, b) = (usize::from_str_radix(a, 16).ok()?, usize::from_str_radix(b, 16).ok()?);
            (a <= addr && addr + data.len() <= b).then(|| (b, rest.split(' ').next().unwrap_or("")))
        })
        .ok_or_else(|| format!("{addr:#x} is not in one mapping"))?;
    if perms.contains('x') {
        bail!("{addr:#x} is in executable memory");
    }
    let page = addr & !(PAGE - 1);
    let len = (addr + data.len() - page).next_multiple_of(PAGE).min(end - page);
    let old = (if perms.starts_with('r') { PROT_READ } else { 0 })
        | (if perms.get(1..2) == Some("w") { PROT_WRITE } else { 0 });
    // SAFETY: `page..page+len` lies in one mapping nothing uses (caller); its permissions are restored after.
    unsafe {
        if mprotect(page as *mut c_void, len, PROT_READ | PROT_WRITE) != 0 {
            bail!("mprotect failed at {addr:#x}");
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
        mprotect(page as *mut c_void, len, old);
    }
    Ok(())
}
