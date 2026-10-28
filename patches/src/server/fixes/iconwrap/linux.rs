//! `iconwrap`, Linux: the render function's store of the offset becomes a call to [`super::store_offset`].

use crate::base::code::branch;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // Render function (ebx = icon record: [+8] list, [+0Ch] its length, [+14h] 16-bit rendered offset, [+18h]
    // rendered copy): lea esi,[ebx+8] ; push esi ; call is_flat ; add esp,10h ; test al,al ; jne done ;
    // mov edx,[world] ; movzx eax,word [ebx+14h] ; xor ecx,ecx ; mov edi,[edx+context] ; test ax,ax ;
    // mov [ebp-30h],edx ; setne cl ; mov [ebp-2Ch],ecx ; test edi,edi ; je make_context ; sub esp,8 ; push 0 ;
    // push [ebp-2Ch] ; push edi ; push [ebx+18h] ; push eax ; push esi ; call run ; then the store:
    // mov eax,[ebx+0Ch] ; add esp,20h ; mov [ebx+14h],ax ; and return. Not the inlined copy in render-and-flatten.
    let at = core.find_unique(
        "8D 73 08 56 E8 ?? ?? ?? ?? 83 C4 10 84 C0 75 ?? 8B 15 ?? ?? ?? ?? 0F B7 43 14 31 C9 8B BA ?? ?? ?? ?? \
         66 85 C0 89 55 D0 0F 95 C1 89 4D D4 85 FF 74 ?? 83 EC 08 6A 00 FF 75 D4 57 FF 73 18 50 56 E8 ?? ?? ?? ?? \
         8B 43 0C 83 C4 20 66 89 43 14 8D 65 F4 5B 5E 5F 5D C3",
        "icon operation list render",
    )?;
    let store = at + 69;
    // is_flat (cdecl, the list): whether the list is the single `?` entry, which render skips.
    let is_flat = core.rel32(at + 5)?;
    if !core.is_at(is_flat, "8B 54 24 04 31 C0 8B 4A 04 85 C9 74 08 8B 02 80 38 3F 0F 94 C0 C3")? {
        bail!("icon operation list: the flat test is not as expected");
    }
    // BYOND's flatten after an operation: call render ; pop ecx ; push [list] ; call flatten ;
    // mov reg,[record+0Ch] ; add esp,10h ; mov [record+14h],ax. Every such site must agree.
    let sites = core.find_all("E8 ?? ?? ?? ?? 59 FF 75 ?? E8 ?? ?? ?? ?? 8B ?? 0C 83 C4 10 66 89 ?? 14")?;
    let Some(first) = sites.first() else { bail!("icon operation list: no flatten after render") };
    let render = core.rel32(first + 1)?;
    let flatten = core.rel32(first + 10)?;
    for &s in &sites {
        if core.rel32(s + 1)? != render || core.rel32(s + 10)? != flatten {
            bail!("icon operation list: flatten sites call different functions");
        }
    }
    if !(at - 0x7F..at).contains(&render) {
        bail!("icon operation list: flatten follows another render function");
    }
    // flatten (cdecl, the list): grow the buffer to at least 2 bytes, then length = 1, "?\0"
    let flatten_ida = "56 53 83 EC 04 8B 5C 24 10 8B 53 08 8B 03 83 FA 02 7F ?? B9 02 00 00 00 89 CE 29 D6 \
                       83 C2 32 83 FE 32 0F 43 D1 83 EC 08 89 53 08 52 50 E8 ?? ?? ?? ?? 83 C4 10 89 03 \
                       C7 43 04 01 00 00 00 C6 00 3F 8B 03 C6 40 01 00 83 C4 04 5B 5E C3";
    if !core.is_at(flatten, flatten_ida)? {
        bail!("icon operation list: flatten is not as expected");
    }
    super::FLATTEN.set(flatten);
    // mov eax,[ebx+0Ch] ; add esp,20h ; mov [ebx+14h],ax  ->  add esp,14h ; push ebx ; call store_offset ; nop.
    // esp is 16-byte aligned after the original add, so the call is too; the epilogue's lea esp,[ebp-0Ch] drops the
    // argument. Render returns nothing, so eax, ecx, and edx are free.
    let mut code = vec![0x83, 0xC4, 0x14, 0x53];
    code.extend(branch(0xE8, store + 4, super::store_offset as *const () as usize, 6));
    core.write_code(store, &code)?;
    Ok(format!("render at +{:#x}", render - core.base))
}
