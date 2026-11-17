//! `filtername`, Windows: the filter list add's append and splice calls (cdecl) go to the wrappers in `mod.rs`. From
//! 516.1689 the add's own handling of a name already there is skipped first.

use super::{Append, FOUND, Found, Splice, Tables};
use crate::base::code::{Cond, Foreign};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// The add's end: cmp ax,si (index, count) ; jb replace ; push edi (the filter) ; push [chain] ; call append ;
/// add esp,8 ; jmp done ; replace: push 1 ; lea ecx,[filter] ; push ecx ; push 1 ; push eax ; push [chain] ;
/// call splice ; add esp,14h ; done. The locals are `[ebp+disp8]` or `[ebp+disp32]`.
const SITE: &str = "66 3B C6 72 ?? 57 FF";
/// Its `push [chain]` (FF /6).
const PUSH_CHAIN: usize = 6;
/// After it: call append ; add esp,8 ; jmp done.
const APPEND: &str = "E8 ?? ?? ?? ?? 83 C4 08 EB ??";
const APPEND_LEN: usize = 10;
/// replace: push 1, then `lea ecx,[filter]` (8D /1), then push ecx ; push 1 ; push eax, then `push [chain]`.
const REPLACE: [u8; 2] = [0x6A, 0x01];
const SPLICE_ARGS: [u8; 4] = [0x51, 0x6A, 0x01, 0x50];
/// call splice ; add esp,14h, then done.
const SPLICE: &str = "E8 ?? ?? ?? ?? 83 C4 14";
const SPLICE_LEN: usize = 8;
/// Before it, the add's loop over a chain's filters by name: cmp eax,[chain count] ; jae ; mov ecx,[chains] ;
/// mov eax,[ecx+eax*4] ; test eax,eax ; je ; movzx esi,word [eax] ; mov ecx,[eax+8] (the ids)
const CHAINS: &str = "3B 05 ?? ?? ?? ?? 73 ?? 8B 0D ?? ?? ?? ?? 8B 04 81 85 C0 74 ?? 0F B7 30 8B 48 08";
const CHAIN_COUNT: usize = 2;
const CHAIN_ARRAY: usize = 10;
/// In it, after `mov eax,[state]`, the filter of an id and its name: mov eax,[eax+filters] ; mov eax,[eax+ecx*4] ;
/// jmp ; xor eax,eax ; cmp [eax+30h],ebx
const NAME: &str = "8B 80 ?? ?? ?? ?? 8B 04 88 EB 02 33 C0 39 58 30";
const NAME_FILTERS: usize = 2;
/// A name found: mov eax,[eax+8] (the ids) ; push [eax+edx*4] ; call the filter getter.
const GET_CALL: &str = "8B 40 08 FF 34 90 E8";
const GET_AT: usize = 6;
/// The getter: mov eax,[state] ; id < [eax+filter count] ? [[eax+filters]+id*4] : 0.
const GET: &str =
    "55 8B EC A1 ?? ?? ?? ?? 8B 4D 08 3B 88 ?? ?? ?? ?? 73 0B 8B 80 ?? ?? ?? ?? 8B 04 88 5D C3 33 C0 5D C3";
const STATE: usize = 4;
const FILTER_COUNT: usize = 13;
const FILTER_ARRAY: usize = 21;

/// 516.1689's end of the add: cmp si,bx (index, count) ; jb replace ; mov edx,[filter] ; push edx ; push [chain] ;
/// call append ; add esp,8 ; jmp done ; replace: push 1 ; lea eax,[filter] ; push eax ; push 1 ; push esi ;
/// push [chain] ; call splice ; add esp,14h ; done
const SITE_1689: &str = "66 3B F3 72 17 8B 95 ?? ?? ?? ?? 52 FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 08 EB 1A 6A 01 \
                         8D 85 ?? ?? ?? ?? 50 6A 01 56 FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 14";
const FILTER_1689: [usize; 2] = [7, 32];
const CHAIN_1689: [usize; 2] = [14, 42];
const APPEND_1689: usize = 18;
const SPLICE_1689: usize = 46;
/// Its check for a name: mov eax,[esi+30h] ; mov [..],eax ; test eax,eax ; je plain ; mov ebx,edi ;
/// cmp eax,0FFFFh ; je plain. `plain` is the add without a name, which ends at the site.
const NAMED_1689: &str = "8B 46 30 89 85 ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 8B DF 3D FF FF 00 00 0F 84";
const NO_NAME_JE: usize = 11;
const NO_NAME_JE_2: usize = 24;
/// Then its lookup of the name: the chain (cmp eax,[chain count] ; jae ; mov ecx,[chains] ; mov eax,[ecx+eax*4] ;
/// test eax,eax ; je ; movzx esi,word [eax] ; mov ecx,[eax+8] (the ids)) ...
const CHAINS_1689: &str = "3B 05 ?? ?? ?? ?? 0F 83 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 8B 04 81 85 C0 0F 84 ?? ?? ?? ?? \
                           0F B7 30 8B 48 08";
const CHAIN_ARRAY_1689: usize = 14;
/// ... the filter count (mov eax,[state] ; mov edx,[name] ; mov eax,[eax+filter count] ; mov ebx,eax) ...
const FILTER_COUNT_1689: &str = "A1 ?? ?? ?? ?? 8B 95 ?? ?? ?? ?? 8B 80 ?? ?? ?? ?? 8B D8";
const FILTER_COUNT_AT_1689: usize = 13;
/// ... and each filter (mov eax,[state] ; mov eax,[eax+filters] ; mov eax,[eax+ecx*4] ; jmp name_cmp).
const FILTERS_1689: &str = "A1 ?? ?? ?? ?? 8B 80 ?? ?? ?? ?? 8B 04 88 EB ??";
const FILTER_ARRAY_AT_1689: usize = 7;
const FILTERS_JMP_1689: usize = 15;
const STATE_1689: usize = 1;
/// name_cmp: cmp [eax+30h],edx
const NAME_CMP_1689: &str = "39 50 30";

/// Length of the `op` at `at` with ModRM reg field `reg` and an `[ebp+disp8]` or `[ebp+disp32]` operand.
fn ebp_operand(core: &Image, at: usize, op: u8, reg: u8) -> Result<usize> {
    let [o, modrm] = core.array(at)?;
    match (o == op).then_some(modrm ^ (reg << 3)) {
        Some(0x45) => Ok(3),
        Some(0x85) => Ok(6),
        _ => bail!("filter list add: {at:#x} not as expected"),
    }
}

/// What install found: the add's end, its append and splice calls, and from 516.1689 the jump to its add without a
/// name.
struct Add {
    site: usize,
    append: usize,
    splice: usize,
    tables: Tables,
    no_name: Option<usize>,
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.find_all(SITE)?;
    let add = if sites.is_empty() { add_1689(core)? } else { add(core, sites.unique("filter list add")?)? };
    let append = Foreign::at(core.call_target(add.append)?);
    let splice = Foreign::at(core.call_target(add.splice)?);
    FOUND.commit(cx, Found { tables: add.tables, append, splice });
    if let Some(at) = add.no_name {
        core.always_jump(at)?;
    }
    core.retarget(add.append + 1, super::append as Append)?;
    core.retarget(add.splice + 1, super::splice as Splice)?;
    Ok(core.report(&[add.site]))
}

/// Through 516.1688: the tables from the add's loop over the chain's names.
fn add(core: &Image, site: usize) -> Result<Add> {
    let replace = core.jcc_at(site + 3, Cond::Below)?.target;
    let chain = core.bytes(site + PUSH_CHAIN, ebp_operand(core, site + PUSH_CHAIN, 0xFF, 6)?)?;
    let append = site + PUSH_CHAIN + chain.len();
    ensure!(append + APPEND_LEN == replace, "filter list add: append not as expected");
    core.require(append, APPEND, "filter list add: append")?;
    let done = core.rel8(append + APPEND_LEN - 1)?;
    core.expect(replace, &REPLACE, "filter list add: replace not as expected")?;
    let args = replace + REPLACE.len() + ebp_operand(core, replace + REPLACE.len(), 0x8D, 1)?;
    core.expect(args, &SPLICE_ARGS, "filter list add: splice arguments not as expected")?;
    core.expect(args + SPLICE_ARGS.len(), chain, "filter list add: splice of another chain")?;
    let splice = args + SPLICE_ARGS.len() + chain.len();
    core.require(splice, SPLICE, "filter list add: splice")?;
    ensure!(splice + SPLICE_LEN == done, "filter list add: splice not as expected");

    let t = core.find_unique(CHAINS, "filter list add's name loop")?;
    ensure!((site - 0x400..site).contains(&t), "filter list add's name loop: not in the add found");
    let name = core.find_unique_in(t, site - t, NAME, "filter list add's name check")?;
    let get = core.find_unique_in(t, site - t, GET_CALL, "filter list add's filter getter call")?;
    let get = core.follow_call(get + GET_AT, GET, "filter getter")?;
    let state = core.ptr_at(get + STATE)?;
    let filter_array = core.ptr_at(get + FILTER_ARRAY)?;
    ensure!(
        core.mentions(t, name - t, state)? && core.ptr_at(name + NAME_FILTERS)? == filter_array,
        "filter list add's name loop: not the getter's filter table"
    );
    let tables = Tables {
        chain_count: core.ptr_at(t + CHAIN_COUNT)?,
        chain_array: core.ptr_at(t + CHAIN_ARRAY)?,
        state,
        filter_count: core.ptr_at(get + FILTER_COUNT)?,
        filter_array,
    };
    Ok(Add { site, append, splice, tables, no_name: None })
}

/// 516.1689: the tables from its inlined lookup of the name, and the jump that skips it.
fn add_1689(core: &Image) -> Result<Add> {
    let site = core.find_unique(SITE_1689, "filter list add")?;
    ensure!(
        core.u32_at(site + FILTER_1689[0])? == core.u32_at(site + FILTER_1689[1])?
            && core.u32_at(site + CHAIN_1689[0])? == core.u32_at(site + CHAIN_1689[1])?,
        "filter list add: append and splice of another filter or chain"
    );
    let named = core.find_unique(NAMED_1689, "filter list add's name check")?;
    let plain = core.jcc_at(named + NO_NAME_JE, Cond::Equal)?.target;
    ensure!(
        core.jcc_at(named + NO_NAME_JE_2, Cond::Equal)?.target == plain
            && named < plain
            && plain < site
            && site - named < 0x400,
        "filter list add's name check: not in the add found"
    );
    let span = plain - named;
    let chains = core.find_unique_in(named, span, CHAINS_1689, "filter list add's name lookup")?;
    let count = core.find_unique_in(named, span, FILTER_COUNT_1689, "filter list add's filter count")?;
    let filters = core.find_unique_in(named, span, FILTERS_1689, "filter list add's filter read")?;
    let state = core.ptr_at(count + STATE_1689)?;
    ensure!(
        core.ptr_at(filters + STATE_1689)? == state
            && core.is_at(core.rel8(filters + FILTERS_JMP_1689)?, NAME_CMP_1689)?,
        "filter list add's name lookup: not as expected"
    );
    let tables = Tables {
        chain_count: core.ptr_at(chains + CHAIN_COUNT)?,
        chain_array: core.ptr_at(chains + CHAIN_ARRAY_1689)?,
        state,
        filter_count: core.ptr_at(count + FILTER_COUNT_AT_1689)?,
        filter_array: core.ptr_at(filters + FILTER_ARRAY_AT_1689)?,
    };
    Ok(Add { site, append: site + APPEND_1689, splice: site + SPLICE_1689, tables, no_name: Some(named + NO_NAME_JE) })
}
