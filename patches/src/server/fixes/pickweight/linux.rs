//! `pickweight`, Linux: the interpreter inlines the opcode, so its final target read is patched in place.

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // After the thresholds: pc += 1 ; eax = code[pc] ; pc = eax - 1 (what follows differs between builds)
    //   movzx eax,[esi+14h] ; add eax,1 ; mov [esi+14h],ax ; movzx eax,ax ;
    //   mov eax,[ecx+eax*4] ; sub eax,1 ; mov [esi+14h],ax
    let at = core.find_unique(
        "0F B7 46 14 83 C0 01 66 89 46 14 0F B7 C0 8B 04 81 83 E8 01 66 89 46 14",
        "weighted pick final target",
    )?;
    let site = at + 14;
    let back = at + 24;
    let [lo, hi] = super::THRESHOLD.to_le_bytes();
    let mut s = Stub::new();
    let ok = s.label();
    let stub = s
        .bytes(&[0x66, 0x81, 0x3C, 0x81, lo, hi]) // cmp word [ecx+eax*4],THRESHOLD
        .jcc8(Cond::Below, ok) // jb ok
        .bytes(&[0x83, 0xC0, 0x01]) // add eax,1
        .bind(ok)
        .bytes(&[0x8B, 0x04, 0x81]) // ok: the replaced code: mov eax,[ecx+eax*4]
        .bytes(&[0x83, 0xE8, 0x01]) // sub eax,1
        .bytes(&[0x66, 0x89, 0x46, 0x14]) // mov [esi+14h],ax
        .jmp(back) // jmp back
        .exec()?;
    core.jump(site, 10, stub)?;
    Ok(core.report(&[at]))
}
