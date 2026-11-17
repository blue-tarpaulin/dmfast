//! `filtername`, Linux: the filter list add's append and splice calls (cdecl) go to the wrappers in `mod.rs`. From
//! 516.1689 the add's own handling of a name already there is skipped first.

use super::{Append, FOUND, Found, Splice, Tables};
use crate::base::code::{Cond, Foreign};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The add's end: cmp [count],dx (index) ; jne replace ; sub esp,8 ; push [filter] ; push eax (the chain) ;
/// call append ; add esp,10h
const SITE: &str = "66 39 95 ?? ?? ?? ?? 0F 85 ?? ?? ?? ?? 83 EC 08 FF B5 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? 83 C4 10";
const APPEND: usize = 23;
/// The same in 516.1689, with a short `jne replace`.
const SITE_1689: &str = "66 39 95 ?? ?? ?? ?? 75 ?? 83 EC 08 FF B5 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? 83 C4 10";
const APPEND_1689: usize = 19;
const JNE: usize = 7;
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
/// 516.1689's: mov ecx,[edi+30h] ; test ecx,ecx ; je plain ; ... ; call lookup (the chain's filter of that name) ;
/// add esp,10h ; cmp ax,-1 ; je plain. `plain` is the add without a name, which ends at the site.
const NAMED_1689: &str = "8B 4F 30 85 C9 0F 84 ?? ?? ?? ?? 31 D2 31 FF 66 31 D2 81 F9 FF FF 00 00 0F 44 CF 52 51 \
                          FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 66 83 F8 FF 0F 84";
const NO_NAME_JE: usize = 5;
const LOOKUP_CALL: usize = 41;
const NOT_FOUND_JE: usize = 53;
/// The lookup: the chain (cmp [chain count],eax ; jbe ; mov edx,[chains] ; ...), then each of its filters by name
/// (mov edx,[state] ; ... ; cmp ecx,[edx+filter count] ; jae ; mov ebp,[edx+filters] ; ... ; cmp [ecx+30h],esi).
const LOOKUP_1689: &str = "55 57 56 53 83 EC 0C 8B 7C 24 2C 8B 74 24 28 89 F8 84 C0 75 ?? 81 FE FF FF 00 00 74 ?? \
                           85 F6 74 ?? 8B 44 24 20 8B 54 24 24 E8 ?? ?? ?? ?? 39 05 ?? ?? ?? ?? 76 ?? 8B 15 ?? ?? ?? ?? \
                           8B 04 82 85 C0 74 ?? 8B 58 08 0F B7 00 8B 15 ?? ?? ?? ?? 83 E8 01 85 F6 74 ?? 66 83 F8 FF \
                           74 ?? 0F B7 C8 8B 0C 8B 3B 8A ?? ?? ?? ?? 0F 83 ?? ?? ?? ?? 8B AA ?? ?? ?? ?? 8D 78 FF \
                           8B 4C 8D 00 39 71 30";
const LOOKUP_CHAIN_COUNT: usize = 48;
const LOOKUP_CHAIN_ARRAY: usize = 56;
const LOOKUP_STATE: usize = 75;
const LOOKUP_FILTER_COUNT: usize = 100;
const LOOKUP_FILTER_ARRAY: usize = 112;

/// What install found: the add's end, its append call, and from 516.1689 the jump to its add without a name.
struct Add {
    site: usize,
    append: usize,
    tables: Tables,
    no_name: Option<usize>,
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.find_all(SITE)?;
    let add = if sites.is_empty() { add_1689(core)? } else { add(core, sites.unique("filter list add")?)? };
    let replace = core.jcc_at(add.site + JNE, Cond::NotEqual)?.target;
    core.require(replace, REPLACE, "filter list add's replace")?;
    let append = Foreign::at(core.call_target(add.append)?);
    let splice = Foreign::at(core.call_target(replace + SPLICE)?);
    FOUND.commit(cx, Found { tables: add.tables, append, splice });
    if let Some(at) = add.no_name {
        core.always_jump(at)?;
    }
    core.retarget(add.append + 1, super::append as Append)?;
    core.retarget(replace + SPLICE + 1, super::splice as Splice)?;
    Ok(core.report(&[add.site]))
}

/// Through 516.1688: the tables from the add's read of the old filter.
fn add(core: &Image, site: usize) -> Result<Add> {
    let t = core.find_unique(TABLES, "filter list add's chain read")?;
    let name = core.find_unique(NAME, "filter list add's name read")?;
    ensure!(
        (site - 0x600..t).contains(&name) && (site - 0x600..site).contains(&t),
        "filter list add's reads: not in the add found"
    );
    Ok(Add {
        site,
        append: site + APPEND,
        tables: Tables {
            chain_count: core.ptr_at(t + CHAIN_COUNT)?,
            chain_array: core.ptr_at(t + CHAIN_ARRAY)?,
            state: core.ptr_at(t + STATE)?,
            filter_count: core.ptr_at(t + FILTER_COUNT)?,
            filter_array: core.ptr_at(t + FILTER_ARRAY)?,
        },
        no_name: None,
    })
}

/// 516.1689: the tables from its lookup of the name, and the jump that skips it.
fn add_1689(core: &Image) -> Result<Add> {
    let site = core.find_unique(SITE_1689, "filter list add")?;
    let named = core.find_unique(NAMED_1689, "filter list add's name check")?;
    let plain = core.jcc_at(named + NO_NAME_JE, Cond::Equal)?.target;
    ensure!(
        core.jcc_at(named + NOT_FOUND_JE, Cond::Equal)?.target == plain
            && named < plain
            && plain < site
            && site - named < 0x800,
        "filter list add's name check: not in the add found"
    );
    let l = core.follow_call(named + LOOKUP_CALL, LOOKUP_1689, "filter list add's name lookup")?;
    Ok(Add {
        site,
        append: site + APPEND_1689,
        tables: Tables {
            chain_count: core.ptr_at(l + LOOKUP_CHAIN_COUNT)?,
            chain_array: core.ptr_at(l + LOOKUP_CHAIN_ARRAY)?,
            state: core.ptr_at(l + LOOKUP_STATE)?,
            filter_count: core.ptr_at(l + LOOKUP_FILTER_COUNT)?,
            filter_array: core.ptr_at(l + LOOKUP_FILTER_ARRAY)?,
        },
        no_name: Some(named + NO_NAME_JE),
    })
}
