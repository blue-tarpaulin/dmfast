//! `contline`, Windows: a stub after the skip loop's `isspace` test ends the loop at a newline.

use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // After `\`: `call getc ; cmp [edi+18h],0Ah ; jne other`, then the loop `call getc ; movzx eax,[edi+18h] ;
    // push eax ; call [isspace] ; add esp,4 ; test eax,eax ; jne loop ; jmp top`. The call at +31 is wildcarded
    // because `ctype` patches it.
    let p = core.find_unique(
        "8B CF E8 ?? ?? ?? ?? 83 7F 18 0A 75 24 66 0F 1F 44 00 00 8B CF E8 ?? ?? ?? ?? 0F B6 47 18 50",
        "backslash newline skip",
    )?;
    if !core.is_at(p + 37, "83 C4 04 85 C0 75 E7 E9")? {
        bail!("backslash newline skip: loop end not as expected");
    }
    let loop_top = p + 19;
    // `jne loop ; jmp top` becomes a jump to the stub, which adds the newline check.
    let at = p + 42;
    let top = core.rel32(at + 3)?;
    let stub = exec_code(17, |s| {
        let mut v = vec![0x74, 0x0A]; // je done
        v.extend([0x83, 0x7F, 0x18, 0x0A]); // cmp dword [edi+18h],0Ah
        v.extend([0x0F, 0x85]); // jne loop
        v.extend((loop_top.wrapping_sub(s + v.len() + 4) as u32).to_le_bytes());
        v.extend(branch(0xE9, s + v.len(), top, 5)); // done: jmp top
        v
    })?;
    core.jump(at, 7, stub)?;
    Ok(format!("at {:#x}", at - core.base))
}
