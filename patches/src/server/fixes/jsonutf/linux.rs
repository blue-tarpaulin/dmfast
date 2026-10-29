//! `jsonutf`, Linux: the branch to the UTF-8 decode becomes a jump to the plain copy.

use crate::base::Result;
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // test bl,bl ; jns copy ; sub esp,8 ; lea eax,[esp+24h] ; push eax ; push edx ; call decode_utf8
    let at =
        core.find_unique("84 DB 0F 89 ?? ?? ?? ?? 83 EC 08 8D 44 24 24 50 52 E8", "AddJsonEncodedUtf8 UTF-8 branch")?;
    // jns copy -> jmp copy
    core.jump(at + 2, 6, core.rel32(at + 4)?)?;
    Ok(format!("at +{:#x}", at - core.base))
}
