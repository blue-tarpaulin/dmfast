//! `procshare`, Linux: libbyond's type proc table builder merges the parent's table through [`merge_stub`] (also when
//! that table is empty), the getter's caller calls [`super::lookup`], and the teardown's loop over types goes through
//! [`teardown_stub`].

use std::arch::naked_asm;

use super::{Found, LOAD, Sigs, Table, commit, find, merge_parent};
use crate::base::Result;
use crate::base::code::Addr;
use crate::base::feature::Ctx;

/// libbyond's merge (regparm: eax = the table built, edx = the one merged in).
static MERGE: Addr = Addr::new();

const SIGS: Sigs = Sigs {
    // The builder (cdecl, type id): returns unless the id is a type whose table isn't built ; for each own proc insert
    // its name and append it (and its verb name, if different) ; mark it built if it has no parent, else build the
    // parent (call builder) and merge its table in unless it is empty, then mark it built: push ebp/edi/esi/ebx ;
    // sub esp,1Ch ; cmp eax,[type count] ; ... ; imul edx,eax,94h ; add ecx,[types] ; ... ; mov edx,[tables] ;
    // lea ebp,[edx+eax*8] ; cmp byte [ebp+6],0 ; ... ; lea edx,[edx+eax*8] ; test edx,edx ; je ;
    // cmp word [edx+4],0 ; je ; mov eax,ebp ; call merge
    builder: "55 57 56 53 83 EC 1C 8B 44 24 30 3B 05 ?? ?? ?? ?? 73 2A 3D FF FF 00 00 74 23 69 D0 94 00 00 00 89 D1 \
        03 0D ?? ?? ?? ?? 89 4C 24 0C 74 0F 8B 15 ?? ?? ?? ?? 8D 2C C2 80 7D 06 00 74 0B 83 C4 1C 5B 5E 5F 5D C3 8D 76 \
        00 8B 41 64 3B 05 ?? ?? ?? ?? 0F 83 B3 00 00 00 8B 15 ?? ?? ?? ?? 8B 3C 82 85 FF 0F 84 A2 00 00 00 66 83 3F 00 \
        0F 84 98 00 00 00 89 6C 24 04 31 DB 8D B4 26 00 00 00 00 90 8B 57 08 0F B7 C3 8B 34 82 3B 35 ?? ?? ?? ?? 0F 83 \
        ?? ?? ?? ?? 6B C6 2C 03 05 ?? ?? ?? ?? 83 EC 04 8B 68 04 8B 40 10 89 44 24 0C 6A 00 55 FF 74 24 10 E8 ?? ?? ?? \
        ?? 83 C4 10 85 C0 74 3C 83 EC 08 56 50 E8 ?? ?? ?? ?? 83 C4 10 3C 01 75 2B 8B 44 24 08 39 C5 74 23 83 EC 04 6A \
        01 50 FF 74 24 10 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 0D 83 EC 08 56 50 E8 ?? ?? ?? ?? 83 C4 10 83 C3 01 66 3B 1F \
        0F 82 7A FF FF FF 8B 6C 24 04 8B 7C 24 0C 8B 47 04 3D FF FF 00 00 75 10 C6 45 06 01 83 C4 1C 5B 5E 5F 5D C3 8D \
        74 26 00 83 EC 0C 50 E8 ?? ?? ?? ?? 8B 47 04 83 C4 10 3B 05 ?? ?? ?? ?? 77 D9 8B 15 ?? ?? ?? ?? 8D 14 C2 85 D2 \
        74 CC 66 83 7A 04 00 74 C5 89 E8 E8",
    builder_globals: [0x0D, 0x24, 0x30],
    proto: 0x94,
    // The getter (cdecl: type id): cmp [type count],edx ; jb null ; mov eax,[tables] ; lea eax,[eax+edx*8].
    getter: "8B 54 24 04 31 C0 39 15 ?? ?? ?? ?? 72 08 A1 ?? ?? ?? ?? 8D 04 D0 C3",
    getter_globals: [8, 0xF],
    // The teardown's loop over types: ... ; add eax,[types] ; push eax ; call ... ; mov eax,[tables] ; add esp,10h ;
    // lea ebp,[eax+edi*8] ; cmp word [ebp+4],0 ; je ; (per entry: FreeShared(values), clear) ; free(entries) ;
    // clear the table ; ... ; cmp [type count],edi ; ja loop
    teardown: "83 EC 0C 31 DB 8B 44 24 18 03 05 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 10 8D 2C F8 66 83 7D \
        04 00 74 39 8D B4 26 00 00 00 00 66 90 0F B7 C3 83 EC 0C 83 C3 01 8D 0C 40 8B 45 00 8D 34 88 FF 76 04 E8 ?? ?? \
        ?? ?? 31 C0 C7 46 04 00 00 00 00 83 C4 10 66 89 46 08 66 3B 5D 04 72 D0 83 EC 0C FF 75 00 83 C7 01 E8 ?? ?? ?? \
        ?? 31 C0 C7 45 00 00 00 00 00 81 44 24 1C 94 00 00 00 83 C4 10 66 89 45 04 C6 45 06 00 39 3D ?? ?? ?? ?? 0F 87",
    teardown_globals: [0x8C, 0x0B, 0x16],
};
/// The builder's skip of an empty parent table, and its merge.
const SKIP_EMPTY: usize = 0x151;
const MERGE_CALL: usize = 0x155;
/// In the teardown: `lea ebp,[eax+edi*8] ; cmp word [ebp+4],0` (eax = the tables), which becomes
/// `call stub ; nop*3`.
const TEARDOWN_SITE: usize = 0x1D;
const TEARDOWN_SITE_LEN: usize = 8;

/// [`merge_parent`] for the stub.
///
/// # Safety
/// libbyond passes the table being built and its parent's.
unsafe extern "C" fn merge_c(dst: *mut Table, src: *const Table) {
    // SAFETY: as the caller's; libbyond's merge.
    unsafe { merge_parent(dst, src, |d, s| merge_regparm(d, s)) }
}

/// Calls libbyond's merge with its regparm arguments.
///
/// # Safety
/// As the merge.
#[unsafe(naked)]
unsafe extern "C" fn merge_regparm(dst: *mut Table, src: *const Table) {
    naked_asm!(
        "mov eax, dword ptr [esp + 4]",
        "mov edx, dword ptr [esp + 8]",
        "jmp dword ptr [{merge}]",
        merge = sym MERGE,
    )
}

/// Called at the builder's merge (eax = the table built, edx = the parent's): [`merge_c`] on an aligned stack.
///
/// # Safety
/// Only called from the builder's merge call.
#[unsafe(naked)]
unsafe extern "C" fn merge_stub() {
    naked_asm!(
        "push ebp",
        "mov ebp, esp",
        "and esp, -16",
        "sub esp, 8",
        "push edx",
        "push eax",
        "call {merge}",
        "mov esp, ebp",
        "pop ebp",
        "ret",
        merge = sym merge_c,
    )
}

/// Called at the teardown's site (eax = the tables, edi = the type id): does what the site did (ebp = the type's
/// table ; cmp word [ebp+4],0) and makes the remembered answers stale. Registers and flags as the site leaves them.
///
/// # Safety
/// Only called from the teardown's site.
#[unsafe(naked)]
unsafe extern "C" fn teardown_stub() {
    naked_asm!(
        "lea ebp, [eax + edi*8]",
        "lock inc dword ptr [{load}]",
        "cmp word ptr [ebp + 4], 0",
        "ret",
        load = sym LOAD,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let Found { builder, teardown, site, globals } = find(core, &SIGS)?;
    let teardown_site = teardown + TEARDOWN_SITE;
    core.untouched(&[(teardown_site, TEARDOWN_SITE_LEN)])?;
    commit(cx, globals, site)?;
    MERGE.set(core.call_target(builder + MERGE_CALL)?);
    core.never_jump(builder + SKIP_EMPTY)?;
    core.retarget(builder + MERGE_CALL + 1, merge_stub as unsafe extern "C" fn())?;
    core.call(teardown_site, TEARDOWN_SITE_LEN, teardown_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[builder + MERGE_CALL, site, teardown]))
}
