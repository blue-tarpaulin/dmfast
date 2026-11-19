//! `procshare`, Windows: byondcore's type proc table builder calls [`merge`] for the parent's table, the getter's
//! caller calls [`super::lookup`], and the teardown's per-type free calls [`free`].

use std::sync::atomic::Ordering::Relaxed;

use super::{Found, LOAD, Sigs, Table, commit, find, merge_parent};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

type Merge = unsafe extern "thiscall" fn(*mut Table, *const Table);
type Free = unsafe extern "fastcall" fn(*mut Table);

static MERGE: Foreign<Merge> = Foreign::new();
static FREE: Foreign<Free> = Foreign::new();

const SIGS: Sigs = Sigs {
    // The builder (cdecl: table, proto): for each own proc insert its name and append it (and its verb name, if
    // different), then build the parent's table if needed and merge it in: push ebp ; mov ebp,esp ; sub esp,8 ; ... ;
    // mov ebx,[ebx+64h] ; cmp ebx,[list count] ; ... ; mov ecx,[ebx+4] ; cmp ecx,0FFFFh ; ... ;
    // mov edx,[type count] ; ... ; imul edi,ecx,98h ; add edi,[types] ; ... ; mov eax,[tables] ; lea esi,[eax+ecx*8] ;
    // ... ; cmp byte [esi+6],0 ; jne ; push edi ; push esi ; call builder ; mov byte [esi+6],1 ; ... ;
    // push parent's table ; call merge ; ... ; push 0 ; call merge
    builder: "55 8B EC 83 EC 08 53 8B 5D 0C 56 57 8B 5B 64 3B 1D ?? ?? ?? ?? 0F 83 7F 00 00 00 A1 ?? ?? ?? ?? \
        8B 1C 98 85 DB 74 73 33 C0 33 F6 66 3B 03 73 6A 8B 43 08 0F B7 CE 8B 3C 88 3B 3D ?? ?? ?? ?? 72 04 33 C0 EB 09 \
        6B C7 2C 03 05 ?? ?? ?? ?? 8B 48 04 8B 40 10 6A 00 89 4D F8 51 8B 4D 08 89 45 FC E8 ?? ?? ?? ?? 85 C0 74 2B 57 \
        8B C8 E8 ?? ?? ?? ?? 84 C0 74 1F 8B 45 FC 39 45 F8 74 17 8B 4D 08 6A 01 50 E8 ?? ?? ?? ?? 85 C0 74 08 57 8B C8 \
        E8 ?? ?? ?? ?? 46 66 3B 33 72 96 8B 5D 0C 8B 4B 04 81 F9 FF FF 00 00 74 6C 8B 15 ?? ?? ?? ?? 3B CA 73 3D 69 F9 \
        98 00 00 00 03 3D ?? ?? ?? ?? 74 2D 3B CA 77 0A A1 ?? ?? ?? ?? 8D 34 C8 EB 02 33 F6 80 7E 06 00 75 17 57 56 E8 \
        ?? ?? ?? ?? C6 46 06 01 83 C4 08 8B 4B 04 8B 15 ?? ?? ?? ?? 3B CA 77 18 A1 ?? ?? ?? ?? 8D 04 C8 8B 4D 08 50 E8 \
        ?? ?? ?? ?? 5F 5E 5B 8B E5 5D C3 8B 4D 08 33 C0 50 E8",
    builder_globals: [0xAA, 0xBA, 0xC5],
    proto: 0x98,
    // The getter (cdecl: type id): cmp ecx,[type count] ; ja null ; mov eax,[tables] ; lea eax,[eax+ecx*8].
    getter: "55 8B EC 8B 4D 08 3B 0D ?? ?? ?? ?? 77 0A A1 ?? ?? ?? ?? 8D 04 C8 5D C3",
    getter_globals: [8, 0xF],
    // The teardown's loop over types: mov eax,[types] ; add eax,ebx ; push eax ; call ... ; mov eax,[tables] ;
    // add esp,4 ; lea ecx,[eax+esi*8] ; call free ; inc esi ; add ebx,98h ; cmp esi,[type count] ; jb loop
    teardown: "A1 ?? ?? ?? ?? 03 C3 50 E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 04 8D 0C F0 E8 ?? ?? ?? ?? 46 81 C3 98 00 \
        00 00 3B 35 ?? ?? ?? ?? 72 D4",
    teardown_globals: [0x26, 1, 0xE],
};
/// The builder's merges: of the parent's table, and of none.
const MERGE_PARENT: usize = 0xFD;
const MERGE_NONE: usize = 0x10F;
/// The teardown's call of the free.
const FREE_CALL: usize = 0x18;
/// The free (fastcall, ecx = table): FreeShared each entry's values, then free the array and clear the table.
const FREE_CODE: &str = "53 56 8B F1 33 C0 33 DB 66 3B 46 04 73 2E 57 90 0F B7 C3 8D 0C 40 8B 06 FF 74 88 04 8D 3C \
    88 E8 ?? ?? ?? ?? 33 C0 C7 47 04 00 00 00 00 43 66 89 47 08 83 C4 04 66 3B 5E 04 72 D5 5F FF 36 FF 15 ?? ?? ?? \
    ?? 83 C4 04 C7 06 00 00 00 00";

/// Replaces the builder's merge of the parent's table (thiscall: this = the table built; the parent's table).
///
/// # Safety
/// BYOND passes the table being built and its parent's.
unsafe extern "thiscall" fn merge(dst: *mut Table, src: *const Table) {
    // SAFETY: as the caller's; BYOND's merge.
    unsafe { merge_parent(dst, src, |d, s| MERGE.get()(d, s)) }
}

/// Replaces the teardown's per-type free: the remembered answers go stale.
///
/// # Safety
/// `table` is a type's table (BYOND passes it).
unsafe extern "fastcall" fn free(table: *mut Table) {
    LOAD.fetch_add(1, Relaxed);
    // SAFETY: BYOND's free, with its argument.
    unsafe { FREE.get()(table) }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let Found { builder, teardown, site, globals } = find(core, &SIGS)?;
    let merge_fn = core.call_target(builder + MERGE_PARENT)?;
    ensure!(core.call_target(builder + MERGE_NONE)? == merge_fn, "type proc table builder: two different merges");
    let free_fn = core.call_target(teardown + FREE_CALL)?;
    core.require(free_fn, FREE_CODE, "type proc table free")?;
    commit(cx, globals, site)?;
    MERGE.set(merge_fn);
    FREE.set(free_fn);
    core.retarget(builder + MERGE_PARENT + 1, merge as Merge)?;
    core.retarget(teardown + FREE_CALL + 1, free as Free)?;
    Ok(core.report(&[builder + MERGE_PARENT, site, teardown]))
}
