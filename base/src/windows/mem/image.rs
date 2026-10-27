//! A loaded PE image ([`Image`]): sections, relocations, imports and exports. Headers and tables are read live;
//! an import slot's value is read as code relied on.

use std::ffi::CStr;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::code::live_u32;
use crate::image::{Image, le32};
use crate::{Result, bail};

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

/// The handle (base address) of the loaded module `name`, 0 if not loaded.
fn module_base(name: &str) -> usize {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated.
    unsafe { GetModuleHandleW(wide.as_ptr()) as usize }
}

/// The `SizeOfImage` of the PE image loaded at `base`.
///
/// # Safety
/// `base` is a loaded PE image (its headers are mapped).
unsafe fn image_size(base: usize) -> usize {
    // SAFETY: the headers are mapped (caller).
    unsafe {
        let nt = base + live_u32(base + 0x3C) as usize;
        live_u32(nt + 0x50) as usize
    }
}

impl Image {
    /// The PE image loaded at `base` (a module handle).
    ///
    /// # Safety
    /// `base` is a loaded PE image, not unloaded while the result exists (install time).
    pub(crate) unsafe fn pe(base: usize) -> Result<Image> {
        // SAFETY: a loaded image's headers are mapped, and the loader maps all of its SizeOfImage bytes (caller).
        let image = unsafe { Image::new(base, vec![(base, image_size(base))]) };
        let sections = image.sections()?;
        image.with_parts(sections)
    }

    /// The program's image (the exe), if its headers parse.
    pub(crate) fn host() -> Option<Image> {
        // SAFETY: the program's image is mapped for the whole process.
        unsafe { Image::pe(GetModuleHandleW(std::ptr::null()) as usize) }.ok()
    }

    /// The loaded module `name` (another DLL, e.g. `ucrtbase.dll`).
    pub(crate) fn loaded(name: &str) -> Result<Image> {
        let base = module_base(name);
        if base == 0 {
            bail!("{name} not loaded");
        }
        // SAFETY: loaded; nothing unloads modules while features install.
        unsafe { Image::pe(base) }
    }

    /// The NT headers' address.
    fn nt(&self) -> Result<usize> {
        Ok(self.base + self.live_u32(self.base + 0x3C)? as usize)
    }

    /// (address, size, executable) of each section.
    fn sections(&self) -> Result<Vec<(usize, usize, bool)>> {
        let nt = self.nt()?;
        let count = (self.live_u32(nt + 4)? >> 16) as usize; // FileHeader.NumberOfSections
        let optional_size = (self.live_u32(nt + 0x14)? & 0xFFFF) as usize; // FileHeader.SizeOfOptionalHeader
        let table = self.live_bytes(nt + 0x18 + optional_size, 40 * count)?;
        Ok(table
            .as_chunks::<40>()
            .0
            .iter()
            .filter_map(|s| {
                let (size, rva, flags) = (le32(s, 8)?, le32(s, 0xC)?, le32(s, 0x24)?);
                Some((self.base + rva as usize, size as usize, flags & IMAGE_SCN_MEM_EXECUTE != 0))
            })
            .collect())
    }

    /// Addresses of every absolute address the loader fixed up (`IMAGE_REL_BASED_HIGHLOW`), in code and data;
    /// empty if the table can't be read.
    pub fn reloc_sites(&self) -> Vec<usize> {
        self.relocs().unwrap_or_default()
    }

    fn relocs(&self) -> Result<Vec<usize>> {
        let nt = self.nt()?;
        // OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC] (PE32).
        let (rva, size) = (self.live_u32(nt + 0xA0)? as usize, self.live_u32(nt + 0xA4)? as usize);
        let table = self.live_bytes(self.base + rva, size)?;
        let mut sites = Vec::new();
        let mut at = 0;
        while let (Some(page), Some(len)) = (le32(table, at), le32(table, at + 4)) {
            let (page, len) = (page as usize, len as usize);
            if len < 8 {
                break;
            }
            for &e in table.get(at + 8..(at + len).min(size)).unwrap_or_default().as_chunks::<2>().0 {
                let e = u16::from_le_bytes(e);
                if e >> 12 == 3 {
                    sites.push(self.base + page + (e & 0xFFF) as usize);
                }
            }
            at += len;
        }
        Ok(sites)
    }

    /// Address of the IAT slot for imported function `name`.
    pub fn iat_slot(&self, name: &str) -> Result<usize> {
        let nt = self.nt()?;
        let mut desc = self.base + self.live_u32(nt + 0x80)? as usize; // OptionalHeader.DataDirectory[IMPORT]
        loop {
            let (int, ft) = (self.live_u32(desc)? as usize, self.live_u32(desc + 16)? as usize);
            if ft == 0 {
                bail!("import {name} not found");
            }
            for i in 0.. {
                let thunk = self.live_u32(self.base + if int != 0 { int } else { ft } + 4 * i)?;
                if thunk == 0 {
                    break;
                }
                if thunk & 0x8000_0000 == 0 && self.c_str(self.base + thunk as usize + 2)? == name.as_bytes() {
                    return Ok(self.base + ft + 4 * i);
                }
            }
            desc += 20;
        }
    }

    /// The addresses of the C runtime's `malloc` and `realloc` this image imports (for [`crate::code::Foreign`]
    /// [`crate::code::MallocFn`] / [`crate::code::ReallocFn`]).
    pub fn crt_alloc(&self) -> Result<(usize, usize)> {
        let malloc = self.ptr_at(self.iat_slot("malloc")?)?;
        Ok((malloc, self.ptr_at(self.iat_slot("realloc")?)?))
    }

    /// Address of the export `name` (byondcore.dll exports C++ methods under decorated names, steadier than
    /// signatures).
    pub fn export(&self, name: &CStr) -> Result<usize> {
        self.open()?;
        // SAFETY: `base` is a loaded module's handle (`Image::pe`); `name` is NUL-terminated.
        unsafe { GetProcAddress(self.base as _, name.as_ptr().cast()) }
            .map(|f| f as usize)
            .ok_or_else(|| format!("no export {}", name.to_string_lossy()).into())
    }
}

/// (start, end) of the loaded module `name`.
pub fn module_range(name: &str) -> Option<(usize, usize)> {
    let base = module_base(name);
    // SAFETY: a loaded module's headers are mapped.
    (base != 0).then(|| (base, base + unsafe { image_size(base) }))
}
