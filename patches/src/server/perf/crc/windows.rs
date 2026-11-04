//! `crc`, Windows: byondcore's CRC hooked by [`entry`].

use super::{CrcFn, FOUND, Found, HOOKED, entry};
use crate::base::Result;
use crate::base::code::{Foreign, trampoline};
use crate::base::feature::Ctx;

/// The CRC: push ebp ; mov ebp,esp ; cmp byte [ready],0 ; jne ; call init ; mov eax,[ebp+8] ; ... ;
/// loop: movzx edx,byte [esi+ebx] ; mov ecx,eax ; shr ecx,18h ; inc esi ; xor edx,ecx ; shl eax,8 ;
/// xor eax,[edx*4+table] ; cmp esi,edi ; jb loop
const CRC: &str = "55 8B EC 80 3D ?? ?? ?? ?? 00 75 05 E8 ?? ?? ?? ?? 8B 45 08 56 57 8B 7D 10 33 F6 85 FF 74 1F 53 8B 5D 0C \
                   0F B6 14 1E 8B C8 C1 E9 18 46 33 D1 C1 E0 08 33 04 95 ?? ?? ?? ?? 3B F7 72 E6 5B 5F 5E 5D C3";
const READY: usize = 5;
const INIT_CALL: usize = 0xC;
const TABLE: usize = 0x35;
/// Its first instructions, moved into the trampoline: push ebp ; mov ebp,esp ; cmp byte [ready],0. Not
/// `Image::hook`: `[ready]` is relocated, which `untouched` refuses.
const ENTRY: usize = 10;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CRC, "CRC-32")?;
    let original = trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?;
    FOUND.commit(
        cx,
        Found {
            table: core.ptr_at(f + TABLE)?,
            ready: core.ptr_at(f + READY)?,
            init: Foreign::at(core.call_target(f + INIT_CALL)?),
            original: Foreign::at(original),
        },
    );
    HOOKED.commit(cx, f);
    core.jump(f, 5, entry as CrcFn)?;
    Ok(core.report(&[f]))
}
