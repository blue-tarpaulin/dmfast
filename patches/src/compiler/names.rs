//! The compiler's name table: its built-in names by id, 1 to a count. Ids move between builds
//! ([`crate::compiler::layout`]).

use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

pub struct Names<'a> {
    core: &'a Image,
    /// Where id 0's entry would be.
    table: usize,
    count: usize,
}

impl<'a> Names<'a> {
    /// The table, from the compiler's interning loop over it.
    #[cfg(windows)]
    pub fn new(core: &'a Image) -> Result<Self> {
        // mov esi,[edx] ; mov ebx,edx ; mov ecx,[eax+names+4] ; mov [ebp-10h],ecx ; test esi,esi
        let at = core.find_unique("8B 32 8B DA 8B 88 ?? ?? ?? ?? 89 4D F0 85 F6", "name table")?;
        // The loop's end: add eax,4 ; mov [ebp-1Ch],eax ; cmp eax,count*4 ; jb loop
        let end = core.find_unique_in(at, 0x300, "83 C0 04 89 45 E4 3D ?? ?? ?? ?? 0F 82", "name table end")?;
        let size = core.ptr_at(end + 7)?;
        ensure!(core.rel32(end + 13)? == at && size % 4 == 0, "name table: not its interning loop");
        Ok(Self { core, table: core.ptr_at(at + 6)? - 4, count: size / 4 })
    }

    /// The table, from the compiler's interning loop over it: push [ebx*4+names+4] ; add ebx,1 ; push esi ;
    /// call intern ; add esp,10h ; cmp ebx,count ; jne ; cmp [esi+88h],count (two inlined copies, one table).
    #[cfg(target_os = "linux")]
    pub fn new(core: &'a Image) -> Result<Self> {
        let hits = core.find_all(
            "FF 34 9D ?? ?? ?? ?? 83 C3 01 56 E8 ?? ?? ?? ?? 83 C4 10 81 FB ?? ?? ?? ?? 75 E2 81 BE 88 00 00 00",
        )?;
        let at = hits.first_or("name table")?;
        let (table, count) = (core.ptr_at(at + 3)?, core.ptr_at(at + 21)?);
        for &h in hits.iter() {
            ensure!(
                core.ptr_at(h + 3)? == table && core.ptr_at(h + 21)? == count,
                "name table: the interning loops read different tables"
            );
        }
        Ok(Self { core, table: table - 4, count })
    }

    /// The name of `id`, if it has one.
    pub fn name(&self, id: u32) -> Result<Option<&'a [u8]>> {
        let id = id as usize;
        if id == 0 || id > self.count {
            return Ok(None);
        }
        let p = self.core.ptr_at(self.table + id * 4)?;
        if p == 0 { Ok(None) } else { self.core.c_str(p).map(Some) }
    }

    /// The id of `name` (the only one).
    pub fn id(&self, name: &str) -> Result<u32> {
        let mut found = None;
        for id in 1..=self.count as u32 {
            if self.name(id)? == Some(name.as_bytes()) {
                ensure!(found.is_none(), "name table: {name} twice");
                found = Some(id);
            }
        }
        let Some(id) = found else { bail!("name table: no {name}") };
        Ok(id)
    }
}
