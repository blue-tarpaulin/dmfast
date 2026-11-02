//! `contline`, Linux: the skip loop's `isspace` test becomes "a space and not a newline".

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // loop: sub esp,0Ch ; push ebx ; call getc ; mov edi,[ebx+18h] ; mov eax,edi ; movzx eax,al ; mov [esp],eax ;
    // call isspace ; add esp,10h ; test eax,eax ; jne loop ; mov ecx,[ebp-30h] ; mov edx,[ebp+8] ; jmp
    let p = core.find_unique(
        "83 EC 0C 53 E8 ?? ?? ?? ?? 8B 7B 18 89 F8 0F B6 C0 89 04 24 E8 ?? ?? ?? ?? 83 C4 10 85 C0 75 E0 \
         8B 4D D0 8B 55 08 E9",
        "backslash newline skip",
    )?;
    let at = p + 25;
    let mut s = Stub::new();
    let done = s.label();
    s.bytes(&[0x83, 0xC4, 0x10]) // add esp,10h
        .bytes(&[0x85, 0xC0]) // test eax,eax
        .jcc8(Cond::Equal, done) // je done
        .bytes(&[0x83, 0xFF, 0x0A]) // cmp edi,0Ah
        .jcc(Cond::NotEqual, p) // jne loop
        .bind(done)
        .jmp(p + 32); // done: jmp after the loop
    let stub = s.exec()?;
    core.jump(at, 7, stub)?;
    Ok(core.report(&[at]))
}
