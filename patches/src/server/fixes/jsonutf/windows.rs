//! `jsonutf`, Windows: the call to the UTF-8 decode becomes a jump to the plain copy.

use crate::base::Result;
use crate::base::feature::Ctx;

/// How far into the function the patched code is searched for.
const SPAN: usize = 0x300;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.export(c"?AddJsonEncodedUtf8@StringEditor@@QAEXPBD_N1@Z")?;
    // cmp bl,80h ; jb ascii ; lea eax,[ebp+0Ch] ; push eax ; push edx ; call decode_utf8
    let decode = core.find_unique_in(
        f,
        SPAN,
        "80 FB 80 0F 82 ?? ?? ?? ?? 8D 45 0C 50 52 E8 ?? ?? ?? ??",
        "AddJsonEncodedUtf8 UTF-8 decode",
    )?;
    // test bl,bl ; je next ; mov eax,[esi+0Ch] ; cmp eax,[esi+8] ; jl room ; push 1 ; mov ecx,esi
    let copy = core.find_unique_in(
        f,
        SPAN,
        "84 DB 0F 84 ?? ?? ?? ?? 8B 46 0C 3B 46 08 7C 14 6A 01 8B CE E8",
        "AddJsonEncodedUtf8 plain copy",
    )?;
    // Over the first 5 bytes of the decode's call sequence; the rest is unreachable.
    core.jump(decode + 9, 5, copy)?;
    Ok(format!("at {:#x}", decode - core.base))
}
