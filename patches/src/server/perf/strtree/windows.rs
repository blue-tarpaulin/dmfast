//! `strtree`, Windows: byondcore's lookup hooked by [`lookup`], and removal's lookup and unlink replaced.

use std::arch::naked_asm;

use super::verify::removed;
use super::{FOUND, Found, LookupFn, SITES, Sites, lookup, remove};
use crate::base::code::{Addr, Foreign, hx, trampoline};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The lookup: (SEH frame) ; xor ebx,ebx ; xor esi,esi ; cmp [count],ebx ; jbe empty ; mov eax,[table] ;
/// mov edi,[eax] ; loop: push edi ; push [ebp+8] ; call compare ; ... ; inc esi ; cmp esi,1000 ; jle step ;
/// cmp [checksum],-1 ; jne rebuild ; xor esi,esi ; step: ... ; mov edi,[ebx] ; test edi,edi ; jne loop
const LOOKUP: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 1C 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
                      64 A3 00 00 00 00 33 DB 33 F6 39 1D ?? ?? ?? ?? 76 4A A1 ?? ?? ?? ?? 8B 38 0F 1F 44 00 00 57 FF 75 08 \
                      E8 ?? ?? ?? ?? 83 C4 08 85 C0 0F 84 ?? ?? ?? ?? 46 81 FE E8 03 00 00 7E 0B 83 3D ?? ?? ?? ?? FF 75 31 \
                      33 F6 33 DB 85 C0 0F 9F C3 8D 1C 9D 08 00 00 00 03 DF 8B 3B 85 FF 75 C2";
const COUNT: usize = 0x2E;
const TABLE: usize = 0x35;
const COMPARE_CALL: usize = 0x44;
const CHECKSUM: usize = 0x5F;
/// On its rebuild path: cmp byte [loading],0.
const LOADING: usize = 0xD4;
/// Its first instructions, moved into the trampoline: push ebp ; mov ebp,esp ; push -1 ; push handler. Not
/// `Image::hook`: the handler's address is relocated, which `untouched` refuses.
const ENTRY: usize = 10;
/// The compare: unsigned bytes, two per iteration, sbb/or for the sign.
const COMPARE: &str = "55 8B EC 8B 45 0C 8B 08 8B 45 08 8B 00 0F 1F 00 8A 10 3A 11 75 1A 84 D2 74 12 8A 50 01 3A 51 01 \
                       75 0E 83 C0 02 83 C1 02 84 D2 75 E4 33 C0 5D C3 1B C0 83 C8 01 5D C3";
/// Removal (esi = entry): lea eax,[ebp+8] ; push eax ; push esi ; call lookup ; add esp,8 ; cmp eax,esi ;
/// je unlink ; (not found) lea eax,[ebp+8] ; push eax ; push esi ; call lookup ; mov eax,[table] ; push edi ;
/// mov [eax+edi*4],0 ; call
const REMOVAL: &str = "8D 45 08 50 56 E8 ?? ?? ?? ?? 83 C4 08 3B C6 74 ?? 8D 45 08 50 56 E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 57 \
                       C7 04 B8 00 00 00 00 E8";
const FIRST_CALL: usize = 5;
/// After the first lookup call (`add esp,8`): the end of what the stubs replace.
const AFTER_CALL: usize = 0xA;
const SECOND_CALL: usize = 0x16;
/// The not-found path after the second lookup call (whose two arguments it pops later): mov eax,[table].
const NOT_FOUND: usize = 0x1B;
/// After the unlink, which ends `pop ebx`, then the same `mov eax,[table] ; push edi ; mov [eax+edi*4],0`.
const UNLINKED: usize = 0xEA;
/// The rebuild (called right before the removal), after its loop: mov ebx,[ebp-10h] ; cmp esi,ebx ; je free (the
/// sort and relink are skipped only for an empty list) ; push [ebp-14h] ; sub esi,ebx ; sar esi,2 ; push esi ;
/// push ebx ; call. Fast mode never reads links, so its `je` always jumps.
const REBUILD_END: &str = "8B 5D F0 3B F3 74 6F FF 75 EC 2B F3 C1 FE 02 56 53 E8";
const REBUILD_SKIP: usize = 0x14B;
const SKIP_JE: usize = 5;

/// Where the removal stubs go on.
static UNLINKED_AT: Addr = Addr::new();
static NOT_FOUND_AT: Addr = Addr::new();
static AFTER_CALL_AT: Addr = Addr::new();

/// Fast mode, in place of removal's `lea eax,[ebp+8] ; push eax ; push esi ; call lookup`: past the unlink if
/// [`remove`] dropped the entry, else the not-found path with two dummy arguments.
///
/// # Safety
/// Only jumped to from removal's first lookup call.
#[unsafe(naked)]
unsafe extern "C" fn remove_stub() {
    naked_asm!(
        "pushad",
        "push esi",
        "call {remove}",
        "add esp, 4",
        "mov dword ptr [esp + 28], eax",
        "popad",
        "test eax, eax",
        "jz 2f",
        "jmp dword ptr [{unlinked}]",
        "2:",
        "push 0",
        "push 0",
        "jmp dword ptr [{not_found}]",
        remove = sym remove,
        unlinked = sym UNLINKED_AT,
        not_found = sym NOT_FOUND_AT,
    )
}

/// Verify mode: removal's own lookup and unlink, with [`removed`] told what the lookup found.
///
/// # Safety
/// Only jumped to from removal's first lookup call.
#[unsafe(naked)]
unsafe extern "C" fn removed_stub() {
    naked_asm!(
        "lea eax, [ebp + 8]",
        "push eax",
        "push esi",
        "call {lookup}",
        "push eax",
        "push eax",
        "push esi",
        "call {removed}",
        "add esp, 8",
        "pop eax",
        "jmp dword ptr [{after}]",
        lookup = sym lookup,
        removed = sym removed,
        after = sym AFTER_CALL_AT,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(LOOKUP, "string tree lookup")?;
    let compare = core.find_unique(COMPARE, "string tree compare")?;
    ensure!(core.call_target(f + COMPARE_CALL)? == compare, "string tree lookup: unexpected compare");
    let (count, table, checksum) = (core.ptr_at(f + COUNT)?, core.ptr_at(f + TABLE)?, core.ptr_at(f + CHECKSUM)?);
    core.require(f + LOADING, "80 3D ?? ?? ?? ?? 00", "string tree lookup loading flag")?;
    let loading = core.ptr_at(f + LOADING + 2)?;

    let r = core.find_unique(REMOVAL, "string removal unlink")?;
    ensure!(
        core.call_target(r + FIRST_CALL)? == f
            && core.call_target(r + SECOND_CALL)? == f
            && core.ptr_at(r + NOT_FOUND + 1)? == table,
        "string removal: unexpected lookup or table"
    );
    let unlink_end = format!("5B A1 {} 57 C7 04 B8 00 00 00 00", hx(table));
    core.require(r + UNLINKED - 1, &unlink_end, "string removal unlink end")?;
    let rebuild = core.call_target(r - 5)?;
    core.require(rebuild + REBUILD_SKIP, REBUILD_END, "string tree rebuild")?;

    let original = trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?;
    FOUND.commit(cx, Found { table, count, checksum, loading, original: Foreign::at(original) });
    SITES.commit(cx, Sites { table, count, loading, lookup: f, rebuild });
    UNLINKED_AT.set(r + UNLINKED);
    NOT_FOUND_AT.set(r + NOT_FOUND);
    AFTER_CALL_AT.set(r + AFTER_CALL);
    if verify() {
        core.jump(r, AFTER_CALL, removed_stub as *const ())?;
    } else {
        core.jump(r, AFTER_CALL, remove_stub as *const ())?;
        core.always_jump(rebuild + REBUILD_SKIP + SKIP_JE)?;
    }
    core.jump(f, 5, lookup as LookupFn)?;
    Ok(core.report(&[f, r]))
}
