//! `filtername`, Windows: the filter list add's append and splice calls (cdecl) go to the wrappers in `mod.rs`.

use super::{Append, FOUND, Found, Splice, Tables};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The add's end: cmp ax,si (index, count) ; jb replace ; push edi (the filter) ; push [ebp-48h] (the chain) ;
/// call append ; add esp,8 ; jmp ; replace: push 1 ; lea ecx,[filter] ; push ecx ; push 1 ; push eax ;
/// push [ebp-48h] ; call splice ; add esp,14h
const SITE: &str = "66 3B C6 72 0E 57 FF 75 B8 E8 ?? ?? ?? ?? 83 C4 08 EB 17 6A 01 8D 8D ?? ?? ?? ?? 51 6A 01 50 FF 75 B8 \
                    E8 ?? ?? ?? ?? 83 C4 14";
const APPEND: usize = 9;
const SPLICE: usize = 34;
/// Before it, the add's loop over a chain's filters by name: cmp eax,[chain count] ; jae ; mov ecx,[chains] ;
/// mov eax,[ecx+eax*4] ; test eax,eax ; je ; movzx esi,word [eax] ; mov ecx,[eax+8] (the ids) ; ... ;
/// mov eax,[state] ; ... ; mov eax,[eax+filter count] ; ... ; mov eax,[state] ; mov eax,[eax+filters] ;
/// mov eax,[eax+ecx*4] ; jmp ; xor eax,eax ; cmp [eax+30h],ebx (the name)
const TABLES: &str = "3B 05 ?? ?? ?? ?? 73 ?? 8B 0D ?? ?? ?? ?? 8B 04 81 85 C0 74 ?? 0F B7 30 8B 48 08 89 8D ?? ?? ?? ?? \
                      66 85 F6 74 ?? A1 ?? ?? ?? ?? 89 BD ?? ?? ?? ?? 8B 80 ?? ?? ?? ?? 89 85 ?? ?? ?? ?? 0F 1F 40 00 \
                      81 C6 FF FF 00 00 0F B7 D6 8B 0C 91 3B 8D ?? ?? ?? ?? 73 ?? A1 ?? ?? ?? ?? 8B 80 ?? ?? ?? ?? \
                      8B 04 88 EB 02 33 C0 39 58 30";
const CHAIN_COUNT: usize = 2;
const CHAIN_ARRAY: usize = 10;
const STATE: [usize; 2] = [39, 86];
const FILTER_COUNT: usize = 51;
const FILTER_ARRAY: usize = 92;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(SITE, "filter list add")?;
    let t = core.find_unique(TABLES, "filter list add's name loop")?;
    ensure!((site - 0x400..site).contains(&t), "filter list add's name loop: not in the add found");
    let state = core.ptr_at(t + STATE[0])?;
    ensure!(core.ptr_at(t + STATE[1])? == state, "filter list add's name loop: two filter tables");
    let tables = Tables {
        chain_count: core.ptr_at(t + CHAIN_COUNT)?,
        chain_array: core.ptr_at(t + CHAIN_ARRAY)?,
        state,
        filter_count: core.ptr_at(t + FILTER_COUNT)?,
        filter_array: core.ptr_at(t + FILTER_ARRAY)?,
    };
    let append = Foreign::at(core.call_target(site + APPEND)?);
    let splice = Foreign::at(core.call_target(site + SPLICE)?);
    FOUND.commit(cx, Found { tables, append, splice });
    core.retarget(site + APPEND + 1, super::append as Append)?;
    core.retarget(site + SPLICE + 1, super::splice as Splice)?;
    Ok(core.report(&[site]))
}
