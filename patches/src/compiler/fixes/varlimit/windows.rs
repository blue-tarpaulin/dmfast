//! `varlimit`, Windows: the id and flag come from the code that builds the `$loc` path, the flags offset from the
//! routine that appends its element.

use super::{Loc, patch};
use crate::base::code::hx;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail};

/// The var reference codegen: cmp byte [eax+10h],12h ; jne ; cmp word [eax+22h],0 ; jne ; push [eax+0Ch] ;
/// call (var path by index) ; add esp,4 ; cmp word [eax+6],1 ; jne ; mov eax,[eax] ; cmp dword [eax],<locIdx> ;
/// jne other
const CHECK: &str = "80 78 10 12 75 ?? 66 83 78 22 00 75 ?? FF 70 0C E8 ?? ?? ?? ?? 83 C4 04 66 83 78 06 01 75 ?? \
                     8B 00 81 38 ?? ?? 00 00 75 ??";
/// The `cmp` and the `jne` after it.
const CMP: usize = 33;
const CMP_LEN: usize = 8;

/// The append routine's element stores: lea edx,[eax+eax*2] ; mov eax,[ebp+8] ; mov [ecx+edx*4],eax (id) ; ... ;
/// mov eax,[ebp+0Ch] ; mov [ecx+edx*4+flags],eax ; ... ; mov eax,[ebp+10h] ; mov [ecx+edx*4+...],eax
const STORES: &str = "8D 14 40 8B 45 08 89 04 91 0F B7 46 06 8B 0E 8D 14 40 8B 45 0C 89 44 91 ?? 0F B7 46 06 8B 0E \
                      8D 14 40 8B 45 10 89 44 91";
const FLAGS_AT: usize = 24;

/// The register flag and the append routine, from the code that builds the `$loc` path:
/// 516: `push edi ; or eax,<flag> ; push eax ; push <locIdx> ; jmp T` ... `T: mov esi,[ebp-x] ; mov ecx,esi ;
/// call append`;
/// 515: `mov eax,<locIdx> ; jmp L` ... `L: mov ecx,<flags> ; push 0 ; push ecx ; mov ecx,[ebp-x] ; push eax ;
/// call append`.
fn loc_flag(core: &Image, id: u32) -> Result<(u32, usize)> {
    let id = hx(id as usize);
    let new = core.find_all(&format!("57 83 C8 ?? 50 68 {id} E9"))?;
    if let [at] = new.as_slice()[..] {
        let t = core.rel32(at + 11)?;
        if !core.is_at(t, "8B B5 ?? ?? ?? ?? 8B CE E8")? {
            bail!("$loc var path: no append call after it");
        }
        let [flag] = core.array(at + 3)?;
        return Ok((u32::from(flag), core.rel32(t + 9)?));
    }
    let mut old = Vec::new();
    for at in core.find_all(&format!("B8 {id} E9"))? {
        let l = core.rel32(at + 6)?;
        if core.is_at(l, "B9 ?? ?? ?? ?? 6A 00 51 8B 8D ?? ?? ?? ?? 50 E8")? {
            old.push(l);
        }
    }
    let [l] = old[..] else {
        bail!("$loc var path: found {} (516 form) and {} (515 form)", new.len(), old.len());
    };
    Ok((core.u32_at(l + 1)?, core.rel32(l + 16)?))
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let cmp = core.find_unique(CHECK, "var reference $loc check")? + CMP;
    let id = core.u32_at(cmp + 2)?;
    let (flag, append) = loc_flag(core, id)?;
    let [flags_at] = core.array(core.find_unique_in(append, 0x80, STORES, "var path element stores")? + FLAGS_AT)?;
    let other = core.rel8(cmp + CMP_LEN - 1)?;
    patch(core, cmp, CMP_LEN, other, Loc { id, flag, flags_at })
}
