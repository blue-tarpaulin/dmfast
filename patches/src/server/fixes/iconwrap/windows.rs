//! `iconwrap`, Windows: the render function's store of the offset becomes a call to [`super::store_offset`].

use crate::base::code::branch;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // The render function (esi = the icon record: [+8] list, [+0Ch] its length, [+14h] the 16-bit rendered
    // offset, [+18h] the rendered copy): cmp dword [esi+0Ch],0 ; je done ; lea edi,[esi+8] ; mov ecx,edi ;
    // call is_flat ; test al,al ; jne done ; cmp word [esi+14h],0 ; mov ecx,[world] ; setne al ; push 0 ;
    // movzx eax,al ; push eax ; lea ecx,[ecx+300h] ; call context ; push eax ; push [esi+18h] ;
    // movzx eax,word [esi+14h] ; push eax ; push edi ; call run ; then the store: mov ax,[esi+0Ch] ;
    // add esp,18h ; mov [esi+14h],ax
    let at = core.find_unique(
        "83 7E 0C 00 74 ?? 8D 7E 08 8B CF E8 ?? ?? ?? ?? 84 C0 75 ?? 66 83 7E 14 00 8B 0D ?? ?? ?? ?? 0F 95 C0 \
         6A 00 0F B6 C0 50 8D 89 ?? ?? ?? ?? E8 ?? ?? ?? ?? 50 FF 76 18 0F B7 46 14 50 57 E8 ?? ?? ?? ?? \
         66 8B 46 0C 83 C4 18 66 89 46 14",
        "icon operation list render",
    )?;
    let store = at + 66;
    // is_flat: whether the list is the single `?` entry, which render skips.
    // cmp dword [ecx+4],0 ; je no ; mov eax,[ecx] ; cmp byte [eax],3Fh ; jne no ; mov al,1 ; ret ; no: xor al,al ; ret
    let is_flat = core.rel32(at + 12)?;
    if !core.is_at(is_flat, "83 79 04 00 74 0A 8B 01 80 38 3F 75 03 B0 01 C3 32 C0 C3")? {
        bail!("icon operation list: the flat test is not as expected");
    }
    // BYOND's render-and-flatten method: push esi ; mov esi,ecx ; call render ; lea ecx,[esi+8] ;
    // call flatten ; mov ax,[esi+0Ch] ; mov [esi+14h],ax ; pop esi ; ret
    let both = core.find_unique(
        "56 8B F1 E8 ?? ?? ?? ?? 8D 4E 08 E8 ?? ?? ?? ?? 66 8B 46 0C 66 89 46 14 5E C3",
        "icon operation list flatten",
    )?;
    let render = core.rel32(both + 4)?;
    if !(at - 0x7F..at).contains(&render) {
        bail!("icon operation list: the flatten method renders with another function");
    }
    // flatten (fastcall, ecx = the list): grow the buffer to at least 2 bytes, then length = 1, "?\0"
    let flatten = core.rel32(both + 12)?;
    let flatten_ida = "56 8B F1 BA 02 00 00 00 8B 4E 08 3B CA 7F ?? 8B C2 2B C1 83 F8 32 73 03 8D 51 32 52 \
                       FF 36 89 56 08 FF 15 ?? ?? ?? ?? 83 C4 08 89 06 8B 06 C7 46 04 01 00 00 00 C6 00 3F \
                       8B 06 5E C6 40 01 00 C3";
    if !core.is_at(flatten, flatten_ida)? {
        bail!("icon operation list: flatten is not as expected");
    }
    super::FLATTEN.set(flatten);
    // mov ax,[esi+0Ch] ; add esp,18h ; mov [esi+14h],ax  ->  push esi ; call store_offset ; add esp,1Ch ; nop x2.
    // Render returns nothing, so eax, ecx, and edx are free.
    let mut code = vec![0x56];
    code.extend(branch(0xE8, store + 1, super::store_offset as *const () as usize, 5));
    code.extend([0x83, 0xC4, 0x1C, 0x90, 0x90]);
    core.write_code(store, &code)?;
    Ok(format!("render at {:#x}", render - core.base))
}
