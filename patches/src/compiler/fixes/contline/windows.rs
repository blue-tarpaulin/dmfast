//! `contline`, Windows: a stub after the skip loop's `isspace` test ends the loop at a newline.

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // After `\`: `call getc ; cmp [edi+18h],0Ah ; jne other`, then the loop `call getc ; movzx eax,[edi+18h] ;
    // push eax ; call [isspace] ; add esp,4 ; test eax,eax ; jne loop ; jmp top`. The call at +31 is wildcarded
    // because `ctype` patches it.
    let p = core.find_unique(
        "8B CF E8 ?? ?? ?? ?? 83 7F 18 0A 75 24 66 0F 1F 44 00 00 8B CF E8 ?? ?? ?? ?? 0F B6 47 18 50",
        "backslash newline skip",
    )?;
    core.require(p + 37, "83 C4 04 85 C0 75 E7 E9", "backslash newline skip: loop end")?;
    let loop_top = p + 19;
    // `jne loop ; jmp top` becomes a jump to the stub, which adds the newline check.
    let at = p + 42;
    let top = core.branch_at(at + 2)?.target;
    let mut s = Stub::new();
    let done = s.label();
    s.jcc8(Cond::Equal, done) // je done
        .bytes(&[0x83, 0x7F, 0x18, 0x0A]) // cmp dword [edi+18h],0Ah
        .jcc(Cond::NotEqual, loop_top) // jne loop
        .bind(done)
        .jmp(top); // done: jmp top
    let stub = s.exec()?;
    core.jump(at, 7, stub)?;
    Ok(core.report(&[at]))
}
