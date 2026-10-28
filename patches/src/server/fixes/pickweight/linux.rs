//! `pickweight`, Linux: the interpreter inlines the opcode, so its final target read is patched in place.

use crate::base::Result;
use crate::base::code::{branch, exec_code};
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
    let stub = exec_code(26, |s| {
        let [lo, hi] = super::THRESHOLD.to_le_bytes();
        let mut v = vec![0x66, 0x81, 0x3C, 0x81, lo, hi]; // cmp word [ecx+eax*4],THRESHOLD
        v.extend([0x72, 0x03, 0x83, 0xC0, 0x01]); // jb ok ; add eax,1
        v.extend([0x8B, 0x04, 0x81, 0x83, 0xE8, 0x01, 0x66, 0x89, 0x46, 0x14]); // ok: the replaced code
        v.extend(branch(0xE9, s + v.len(), back, 5)); // jmp back
        v
    })?;
    core.jump(site, 10, stub)?;
    Ok(format!("at +{:#x}", at - core.base))
}
