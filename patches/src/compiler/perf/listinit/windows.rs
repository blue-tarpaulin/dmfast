//! `listinit`, Windows: byondcore's list initializer intern jumps to the replacement; a trampoline runs the stock
//! one.

use super::{Found, init, intern};
use crate::base::code::{Foreign, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // push ebp ; mov ebp,esp ; sub esp,8 ; mov ecx,[ebp+8] ; push ebx ; mov ebx,[count] ;
    // mov [ebp-4],ebx ; push esi ; push edi ; test ecx,ecx ; je ... ; movzx edx,word [ecx+4]
    let f = core.find_unique(
        "55 8B EC 83 EC 08 8B 4D 08 53 8B 1D ?? ?? ?? ?? 89 5D FC 56 57 85 C9 74 ?? 0F B7 51 04",
        "list initializer intern",
    )?;
    let count = core.ptr_at(f + 12)?;
    // The entry array is the dword right before the count; the function must use it.
    let array = count - 4;
    if !core.mentions(f, 0x180, array)? {
        bail!("list initializer intern: entry array not where expected");
    }
    let original = trampoline(core.bytes(f, 6)?, f + 6)?;
    let (malloc, realloc) = core.crt_alloc()?;
    init(Found {
        array,
        count,
        malloc: Foreign::at(malloc),
        realloc: Foreign::at(realloc),
        original: Foreign::at(original),
    });
    core.jump(f, 5, intern as extern "C" fn(usize) -> u32)?;
    Ok(format!("at {:#x}", f - core.base))
}
