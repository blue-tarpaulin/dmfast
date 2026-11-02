//! `varlimit`, Linux: the id comes from the check itself, the flag from the code that stores the `$loc` element (a
//! constant on 515, an `or` on 516); the flags field is at +4.

use super::{Loc, patch};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// The var reference codegen: cmp word [eax+1Eh],0 ; jne ; sub esp,0Ch ; push [eax+0Ch] ; call (var path by index) ;
/// add esp,10h ; cmp word [eax+6],1 ; jne ; mov eax,[eax] ; cmp dword [eax],<locIdx> ; jne other
const CHECK: &str = "66 83 78 1E 00 0F 85 ?? ?? ?? ?? 83 EC 0C FF 70 0C E8 ?? ?? ?? ?? 83 C4 10 66 83 78 06 01 0F 85 \
                     ?? ?? ?? ?? 8B 00 81 38 ?? ?? 00 00 0F 85 ?? ?? ?? ??";
/// The `cmp` and the `jne` after it.
const CMP: usize = 38;
const CMP_LEN: usize = 12;

/// The element stores of var paths: lea eax,[eax+eax*2] ; mov dword [ecx+eax*4],<id> ; movzx eax,word [edi+6]. The
/// one storing `locIdx` builds `$loc`; the others are only candidates (not relied on).
const STORE: &str = "8D 04 40 C7 04 81 ?? ?? 00 00 0F B7 47 06";

/// The register flag, after the `$loc` element's store: a constant (`mov dword [eax+4],<flags>`), or esi
/// (`mov [eax+4],esi`) after an `or esi,<flag>` before it.
fn loc_flag(core: &Image, id: u32) -> Result<u32> {
    let store = core.probe_unique(STORE, "$loc element store", |s| {
        core.array::<2>(s + 6).is_ok_and(|b| u32::from(u16::from_le_bytes(b)) == id)
    })?;
    let (after, before) = (store + 10, store - 0x30);
    if let Some(at) = core.find_bytes_in(after, 0x20, &[0xC7, 0x40, 0x04])?.first() {
        return core.u32_at(at + 3);
    }
    ensure!(!core.find_bytes_in(after, 0x20, &[0x89, 0x70, 0x04])?.is_empty(), "no flags store after the $loc element");
    let Some(&or) = core.find_bytes_in(before, 0x30, &[0x83, 0xCE])?.as_slice().last() else {
        bail!("no `or esi,<flag>` before the $loc store");
    };
    let flag = core.u8_at(or + 2)?;
    Ok(u32::from(flag))
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let cmp = core.find_unique(CHECK, "var reference $loc check")? + CMP;
    let id = core.u32_at(cmp + 2)?;
    let flag = loc_flag(core, id)?;
    let other = core.branch_at(cmp + 6)?.target;
    patch(core, cmp, CMP_LEN, other, Loc { id, flag, flags_at: 4 })
}
