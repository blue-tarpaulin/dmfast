//! `filtername`, Linux: the filter list add's append and splice calls (cdecl) go to the wrappers in `mod.rs`.

use super::{Append, FOUND, Found, Splice, Tables};
use crate::base::code::{Cond, Foreign};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The add's end: cmp [count],dx (index) ; jne replace ; sub esp,8 ; push [filter] ; push eax (the chain) ;
/// call append ; add esp,10h
const SITE: &str = "66 39 95 ?? ?? ?? ?? 0F 85 ?? ?? ?? ?? 83 EC 08 FF B5 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? 83 C4 10";
const JNE: usize = 7;
const APPEND: usize = 23;
/// replace: sub esp,0Ch ; lea edx,[filter] ; push 1 ; push edx ; movzx edx,[index] ; push 1 ; push edx ;
/// push eax (the chain) ; call splice ; add esp,20h
const REPLACE: &str = "83 EC 0C 8D 95 ?? ?? ?? ?? 6A 01 52 0F B7 95 ?? ?? ?? ?? 6A 01 52 50 E8 ?? ?? ?? ?? 83 C4 20";
const SPLICE: usize = 23;
/// Before it, a filter of the appearance's chain: mov edx,[ebx+0B8h] (the chain) ; cmp edx,[chain count] ; jae ;
/// mov ecx,[chains] ; ... ; mov edx,[ecx+edx*4] ; mov ecx,[state] ; mov eax,[edx+8] (the ids) ;
/// mov edx,[eax+edi*4] ; xor eax,eax ; cmp edx,[ecx+filter count] ; jae ; mov eax,[ecx+filters] ;
/// mov eax,[eax+edx*4]
const TABLES: &str = "8B 93 B8 00 00 00 3B 15 ?? ?? ?? ?? 0F 83 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 0F B7 F8 89 BD ?? ?? ?? ?? \
                      8B 14 91 8B 0D ?? ?? ?? ?? 8B 42 08 8B 14 B8 31 C0 3B 91 ?? ?? ?? ?? 73 ?? 8B 81 ?? ?? ?? ?? \
                      8B 04 90";
const CHAIN_COUNT: usize = 8;
const CHAIN_ARRAY: usize = 20;
const STATE: usize = 38;
const FILTER_COUNT: usize = 52;
const FILTER_ARRAY: usize = 60;
/// And before that, the added filter's name: mov ecx,[eax+30h] ; test ecx,ecx ; je ; ... ; cmp ecx,0FFFFh ;
/// cmove ecx,eax
const NAME: &str = "8B 48 30 85 C9 0F 84 ?? ?? ?? ?? 31 D2 31 C0 66 31 D2 81 F9 FF FF 00 00 0F 44 C8";

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(SITE, "filter list add")?;
    let replace = core.jcc_at(site + JNE, Cond::NotEqual)?.target;
    core.require(replace, REPLACE, "filter list add's replace")?;
    let t = core.find_unique(TABLES, "filter list add's chain read")?;
    let name = core.find_unique(NAME, "filter list add's name read")?;
    ensure!(
        (site - 0x600..t).contains(&name) && (site - 0x600..site).contains(&t),
        "filter list add's reads: not in the add found"
    );
    let tables = Tables {
        chain_count: core.ptr_at(t + CHAIN_COUNT)?,
        chain_array: core.ptr_at(t + CHAIN_ARRAY)?,
        state: core.ptr_at(t + STATE)?,
        filter_count: core.ptr_at(t + FILTER_COUNT)?,
        filter_array: core.ptr_at(t + FILTER_ARRAY)?,
    };
    let append = Foreign::at(core.call_target(site + APPEND)?);
    let splice = Foreign::at(core.call_target(replace + SPLICE)?);
    FOUND.commit(cx, Found { tables, append, splice });
    core.retarget(site + APPEND + 1, super::append as Append)?;
    core.retarget(replace + SPLICE + 1, super::splice as Splice)?;
    Ok(core.report(&[site]))
}
