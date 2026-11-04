//! `filtername`, Windows: the filter list add's append and splice calls (cdecl) go to the wrappers in `mod.rs`.

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

/// Length of the `op` at `at` with ModRM reg field `reg` and an `[ebp+disp8]` or `[ebp+disp32]` operand.
fn ebp_operand(core: &Image, at: usize, op: u8, reg: u8) -> Result<usize> {
    let [o, modrm] = core.array(at)?;
    match (o == op).then_some(modrm ^ (reg << 3)) {
        Some(0x45) => Ok(3),
        Some(0x85) => Ok(6),
        _ => bail!("filter list add: {at:#x} not as expected"),
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(SITE, "filter list add")?;
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
    let append_fn = Foreign::at(core.call_target(append)?);
    let splice_fn = Foreign::at(core.call_target(splice)?);
    FOUND.commit(cx, Found { tables, append: append_fn, splice: splice_fn });
    core.retarget(append + 1, super::append as Append)?;
    core.retarget(splice + 1, super::splice as Splice)?;
    Ok(core.report(&[site]))
}
