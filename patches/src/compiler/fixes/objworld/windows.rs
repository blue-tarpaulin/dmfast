//! `objworld`, Windows: the world node's entry pass and the var-name dispatch of an entry (node in eax in both).

use super::{EAX, Site, parent_offset, redirect};
use crate::base::Result;
use crate::base::code::Cond;
use crate::base::feature::Ctx;

/// The entry pass for a `world` node: mov eax,[edi+0Ch] ; add esp,1Ch ; cmp dword [eax],13h ; jne skip ;
/// mov eax,[edi+8] ; test eax,eax ; je ; xor ecx,ecx ; xor ebx,ebx ; cmp cx,[eax+66h]
const PASS: &str = "8B 47 0C 83 C4 1C 83 38 13 75 36 8B 47 08 85 C0 74 2F 33 C9 33 DB 66 3B 48 66";
const PASS_CMP: usize = 6;
/// The var-name dispatch of an entry: cmp dword [eax],13h ; mov ecx,[ebp-9Ch] ; mov ecx,[esi+ecx] ; jne other ;
/// cmp ecx,13Ah ; ja
const DISPATCH: &str = "83 38 13 8B 8D 64 FF FF FF 8B 0C 0E 0F 85 ?? ?? ?? ?? 81 F9 3A 01 00 00 0F 87";

/// The type path builder: push ebp ; mov ebp,esp ; push ecx ; push edi ; mov edi,[ebp+8] (node) ; test edi,edi ;
/// je ; mov ecx,[ebp+10h] ; push ebx ; mov ebx,[ebp+0Ch] ; push ecx ; push ebx ; push [edi+parent] ; call itself.
/// Matches that call something else are not relied on.
const BUILDER: &str =
    "55 8B EC 51 57 8B 7D 08 85 FF 0F 84 ?? ?? ?? ?? 8B 4D 10 53 8B 5D 0C 51 53 FF 77 ?? E8 ?? ?? ?? ??";
const PARENT: usize = 27;
const RECURSE: usize = 29;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let p = core.find_unique(PASS, "world node test")? + PASS_CMP;
    let q = core.find_unique(DISPATCH, "world var dispatch")?;
    let parent = parent_offset(core, BUILDER, PARENT, RECURSE)?;
    // jne skip, re-encoded.
    redirect(
        core,
        Site { at: p, node: EAX, copy: 0, jcc: Some((Cond::NotEqual, core.rel8(p + 4)?)), back: p + 5 },
        parent,
    )?;
    // mov ecx,[ebp-9Ch] copied; the jne after it stays.
    redirect(core, Site { at: q, node: EAX, copy: 6, jcc: None, back: q + 9 }, parent)?;
    Ok(format!("at +{:#x}, +{:#x}", p - core.base, q - core.base))
}
