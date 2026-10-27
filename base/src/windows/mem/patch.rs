//! Writing code: in-place patches and branches ([`crate::ledger`] does [`crate::image::Image`]'s recorded writes
//! with [`poke_code`]), and the executable memory behind [`crate::code::exec_code`].

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc,
    VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::image::Image;
use crate::{Result, bail};

impl Image {
    /// Turn `call dword ptr [slot]` (FF 15 imm32) at `site` into `call target ; nop`.
    pub fn call_iat_to(&self, site: usize, slot: usize, target: usize) -> Result<()> {
        if self.array(site)? != [0xFF, 0x15] || self.ptr_at(site + 2)? != slot {
            bail!("expected call [iat] at {site:#x}");
        }
        self.call(site, 6, target)
    }
}

/// Write `data` at `addr` in an image, through its page protection.
///
/// # Safety
/// No thread may be executing or reading `addr..addr+data.len()`; the new bytes must be valid code (or data).
pub(crate) unsafe fn poke_code(addr: usize, data: &[u8]) -> Result<()> {
    // SAFETY: `addr..addr+data.len()` lies in a loaded image and nothing uses it (caller).
    unsafe {
        let mut old = 0u32;
        if VirtualProtect(addr as _, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            bail!("VirtualProtect failed at {addr:#x}");
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
        VirtualProtect(addr as _, data.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), addr as _, data.len());
    }
    Ok(())
}

/// `len` fresh writable bytes, to fill and [`seal_exec`].
pub(crate) fn alloc_exec(len: usize) -> Result<usize> {
    // SAFETY: fresh memory; nothing else uses it.
    let p = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) } as usize;
    if p == 0 { Err("VirtualAlloc failed".into()) } else { Ok(p) }
}

/// Make an [`alloc_exec`] block executable (no longer writable).
///
/// # Safety
/// `p..p+len` is one [`alloc_exec`] block.
pub(crate) unsafe fn seal_exec(p: usize, len: usize) -> Result<()> {
    // SAFETY: `p..p+len` is one `alloc_exec` block (caller).
    unsafe {
        let mut old = 0u32;
        if VirtualProtect(p as _, len, PAGE_EXECUTE_READ, &mut old) == 0 {
            return Err("VirtualProtect failed".into());
        }
        FlushInstructionCache(GetCurrentProcess(), p as _, len);
    }
    Ok(())
}

/// Free an [`alloc_exec`] block.
///
/// # Safety
/// `p` is an [`alloc_exec`] block that nothing runs or points into.
pub(crate) unsafe fn free_exec(p: usize, _len: usize) {
    // SAFETY: `p` is an `alloc_exec` block nothing uses (caller).
    unsafe { VirtualFree(p as _, 0, MEM_RELEASE) };
}
