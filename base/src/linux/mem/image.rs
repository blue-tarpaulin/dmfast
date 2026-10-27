//! A loaded ELF object ([`Image`]'s Linux part): finding it, its segments, relocations and exports, and calls to
//! its imports.

use std::ffi::{CStr, CString, c_int, c_void};

use libc::{PF_X, PT_DYNAMIC, PT_LOAD, RTLD_LAZY, RTLD_NOLOAD, dl_iterate_phdr, dl_phdr_info, dlclose, dlopen, dlsym};

use crate::image::{Image, le32};
use crate::{Result, bail};

impl Image {
    /// The main program.
    pub(crate) fn host() -> Option<Image> {
        // dl_iterate_phdr reports the main program first, with an empty name.
        Self::first(|path| path.is_empty())
    }

    /// The loaded shared object whose path ends with `/<name>`.
    pub(crate) fn loaded(name: &str) -> Result<Image> {
        Self::first(|path| path.rsplit(|&b| b == b'/').next() == Some(name.as_bytes()))
            .ok_or_else(|| format!("{name} not loaded").into())
    }

    /// Address of the dynamic symbol `name` defined in this object.
    pub fn export(&self, name: &CStr) -> Result<usize> {
        self.open()?;
        let missing = || crate::Error::from(format!("no export {}", name.to_string_lossy()));
        // SAFETY: NUL-terminated names; RTLD_NOLOAD only looks up a loaded object (the main program for NULL),
        // which nothing unloads while features install; dlsym only looks `name` up.
        let addr = unsafe {
            let path = self.path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
            let handle = dlopen(path, RTLD_LAZY | RTLD_NOLOAD);
            if handle.is_null() {
                return Err(missing());
            }
            // The object itself first, then its dependencies (the main program: the global scope).
            let addr = dlsym(handle, name.as_ptr()) as usize;
            dlclose(handle);
            addr
        };
        if addr == 0 || !self.is_mapped(addr, 1) {
            return Err(missing());
        }
        Ok(addr)
    }

    /// Every address the loader wrote an absolute 32-bit relocation to (`DT_REL`); empty if there is no table.
    pub fn reloc_sites(&self) -> Vec<usize> {
        const R_386_32: u32 = 1;
        const R_386_RELATIVE: u32 = 8;
        let Ok((_, _, rel)) = self.dyn_tables() else { return Vec::new() };
        rel.as_chunks()
            .0
            .iter()
            .filter_map(entry)
            .filter(|&(_, info)| matches!(info & 0xFF, R_386_32 | R_386_RELATIVE))
            .map(|(off, _)| self.base + off as usize)
            .collect()
    }

    /// Every `call`/`jmp rel32` site to import `name` (via `R_386_PC32`), each checked to target `target`.
    pub fn import_calls(&self, name: &str, target: usize) -> Result<Vec<usize>> {
        const R_386_PC32: u32 = 2;
        let (strtab, symtab, rel) = self.dyn_tables()?;
        let mut sites = Vec::new();
        for (off, info) in rel.as_chunks().0.iter().filter_map(entry) {
            if info & 0xFF != R_386_PC32 || info >> 8 == 0 {
                continue;
            }
            let st_name = self.live_u32(symtab + (info >> 8) as usize * 16)?;
            if self.c_str(strtab + st_name as usize)? != name.as_bytes() {
                continue;
            }
            let site = self.base + off as usize - 1;
            let [op] = self.array(site)?;
            if !matches!(op, 0xE8 | 0xE9) || self.rel32(site + 1)? != target {
                bail!("{name} site +{:#x}: not a call/jmp to it", site - self.base);
            }
            sites.push(site);
        }
        Ok(sites)
    }

    fn first(matches: impl Fn(&[u8]) -> bool) -> Option<Image> {
        type Found = (usize, Vec<(usize, usize, bool)>, Option<usize>, Option<CString>);
        struct Search<'a> {
            matches: &'a dyn Fn(&[u8]) -> bool,
            found: Option<Found>,
        }
        unsafe extern "C" fn visit(info: *mut dl_phdr_info, _size: usize, data: *mut c_void) -> c_int {
            // SAFETY: dl_iterate_phdr passes a valid entry and our `data`.
            unsafe {
                let s = &mut *(data as *mut Search);
                let info = &*info;
                let path = if info.dlpi_name.is_null() { &[][..] } else { CStr::from_ptr(info.dlpi_name).to_bytes() };
                if !(s.matches)(path) {
                    return 0;
                }
                let base = info.dlpi_addr as usize;
                let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize);
                let segments = phdrs
                    .iter()
                    .filter(|p| p.p_type == PT_LOAD)
                    .map(|p| (base + p.p_vaddr as usize, p.p_memsz as usize, p.p_flags & PF_X != 0))
                    .collect();
                let dynamic = phdrs.iter().find(|p| p.p_type == PT_DYNAMIC).map(|p| base + p.p_vaddr as usize);
                // The main program's empty name: `dlopen(NULL)`.
                let name = (!path.is_empty()).then(|| CString::new(path).ok()).flatten();
                s.found = Some((base, segments, dynamic, name));
                1
            }
        }
        let mut s = Search { matches: &matches, found: None };
        // SAFETY: `visit` only uses `s` during the call.
        unsafe { dl_iterate_phdr(Some(visit), &mut s as *mut Search as *mut c_void) };
        let (base, segments, dynamic, path) = s.found?;
        let mapped = segments.iter().map(|&(a, n, _)| (a, n)).collect();
        // SAFETY: a loaded object's segments are mapped; nothing unloads objects while features install.
        let mut image = unsafe { Image::new(base, mapped) }.with_parts(segments).ok()?;
        image.dynamic = dynamic;
        image.path = path;
        Some(image)
    }

    /// The dynamic section's string table, symbol table and `DT_REL` relocations.
    fn dyn_tables(&self) -> Result<(usize, usize, &[u8])> {
        const DT_STRTAB: u32 = 5;
        const DT_SYMTAB: u32 = 6;
        const DT_REL: u32 = 17;
        const DT_RELSZ: u32 = 18;
        let mut d = self.dynamic.ok_or("no dynamic section")?;
        let (mut strtab, mut symtab, mut rel, mut relsz) = (0, 0, 0, 0);
        // The loader may or may not have rebased the address entries in place.
        let abs = |v: u32| if (v as usize) < self.base { self.base + v as usize } else { v as usize };
        loop {
            let (tag, val) = (self.live_u32(d)?, self.live_u32(d + 4)?);
            match tag {
                0 => break,
                DT_STRTAB => strtab = abs(val),
                DT_SYMTAB => symtab = abs(val),
                DT_REL => rel = abs(val),
                DT_RELSZ => relsz = val as usize,
                _ => {}
            }
            d += 8;
        }
        if strtab == 0 || symtab == 0 || rel == 0 {
            return Err("dynamic section lacks STRTAB/SYMTAB/REL".into());
        }
        Ok((strtab, symtab, self.live_bytes(rel, relsz)?))
    }
}

/// A `DT_REL` entry's offset and info.
fn entry(r: &[u8; 8]) -> Option<(u32, u32)> {
    Some((le32(r, 0)?, le32(r, 4)?))
}
