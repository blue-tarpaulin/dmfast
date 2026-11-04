//! `iconstates`, Windows: the builder's one call of the set goes to [`set_state`]; the stub replaces the scan's
//! first two instructions (`mov eax,[esi+0Ch] ; mov edi,[ebp+10h]`).

use std::arch::naked_asm;

use super::verify::check;
use super::{BUILDING, SET, SetFn, set_state};
use crate::base::code::{Addr, Cond};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The builder: push 0Fh ; push 0 ; call new_list ; ... ; mov [ebp+10h],eax ; ... ; mov [ebp-4],edx (the list) ;
/// loop over the icon's states: skip by the movement flag (test byte [state+10h],4) ; push -1 ; push [state+14h] ;
/// call text ; push 6 ; push eax ; call value ; push [null] x2 ; push edx ; push eax ; push [ebp-4] ; push [ebp+10h] ;
/// call set ; add esp,28h ; next. Nothing else in the loop touches the list.
const BUILDER: &str = "6A 0F 6A 00 E8 ?? ?? ?? ?? 83 C4 04 50 E8 ?? ?? ?? ?? 83 C4 08 89 45 10 80 7F 04 00 89 55 FC 75 0F \
     8B CF E8 ?? ?? ?? ?? 8B 5D 14 83 F8 01 75 05 BB 02 00 00 00 33 F6 39 77 0C 7E ?? 66 0F 1F 44 00 00 8B 47 08 8B 04 \
     B0 F6 40 10 04 74 05 83 FB 01 EB 02 85 DB 74 ?? 6A FF FF 70 14 E8 ?? ?? ?? ?? 6A 06 50 E8 ?? ?? ?? ?? FF 35 ?? ?? \
     ?? ?? FF 35 ?? ?? ?? ?? 52 50 FF 75 FC FF 75 10 E8 ?? ?? ?? ?? 83 C4 28 46 3B 77 0C 7C ??";
/// Its call of the set.
const SITE: usize = 125;
/// The set: push ebp ; mov ebp,esp ; mov al,[ebp+8] ; cmp al,0Fh ; je list (rel8, or rel32). The other types'
/// branches after it aren't used: the builder's list is a plain list.
const SET_HEAD: &str = "55 8B EC 8A 45 08 3C 0F";
/// The list branch hands every argument but the type on to the list set.
const LIST: &str = "FF 75 1C FF 75 18 FF 75 14 FF 75 10 FF 75 0C E8 ?? ?? ?? ?? 83 C4 14 5D C3";
/// The list set: look the key up in the tree ([list+4]); found: replace the value; else the new-key path.
const LIST_SET: &str = "55 8B EC 56 8B 75 08 57 56 E8 ?? ?? ?? ?? 83 C4 04 85 C0 75 ?? 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 \
     C4 04 E8 ?? ?? ?? ?? FF 75 10 FF 75 0C FF 70 04 E8 ?? ?? ?? ?? 8B F8 83 C4 0C 85 FF 74 ?? 8B 57 0C 8B 77 08 8B 45 14 \
     8B 4D 18 52 56 89 47 08 89 4F 0C E8 ?? ?? ?? ?? 83 C4 08 5F 5E 5D C3 FF 75 18 FF 75 14 FF 75 10 FF 75 0C 56 E8";
/// The new-key path: esi = the record ; scan the items from the end for the key ; not found (eax = -1): push 0Fh ;
/// push id ; call value ; append ; then insert into the tree ([esi+4]) and store the value.
const NEW_KEY: &str = "55 8B EC 53 56 57 FF 75 08 E8 ?? ?? ?? ?? 8B F0 83 C4 04 85 F6 75 ?? 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? \
     83 C4 04 E8 ?? ?? ?? ?? 8B F0 8B 46 0C 8B 7D 10 8B 5D 0C 85 C0 74 ?? 8B 0E 66 0F 1F 44 00 00 48 3A 1C C1 75 06 3B \
     7C C1 04 74 05 85 C0 75 F0 48 85 C0 79 ?? 6A 0F FF 75 08 E8 ?? ?? ?? ?? 57 53 52 50 E8 ?? ?? ?? ?? 83 C4 18 57 8D 46 \
     04 53 50 E8 ?? ?? ?? ?? 8B 4D 14 83 C4 0C 8B 55 18 89 48 08 5F 5E 89 50 0C 5B 5D C3";
/// In the new-key path: the scan, the empty list's `jz not_found` field, and not-found (the `dec eax` the loop also
/// falls through to).
const SCAN: usize = 0x2B;
const EMPTY_JZ: usize = 0x37;
const NOT_FOUND_AT: usize = 0x50;

/// The scan's not-found exit and the instruction after the displaced ones.
static NOT_FOUND: Addr = Addr::new();
static BACK: Addr = Addr::new();

/// # Safety
/// Only jumped to from the new-key path's scan.
#[unsafe(naked)]
unsafe extern "C" fn skip_stub() {
    naked_asm!(
        "mov eax, dword ptr [esi + 0x0C]",
        "mov edi, dword ptr [ebp + 0x10]",
        "mov ecx, dword ptr [ebp + 8]",
        "cmp ecx, dword ptr [{building}]",
        "jne 2f",
        // As the scan leaves it when nothing matches: ebx = the key's type, eax = 0 before its `dec eax`.
        "mov ebx, dword ptr [ebp + 0x0C]",
        "xor eax, eax",
        "jmp dword ptr [{not_found}]",
        "2:",
        "jmp dword ptr [{back}]",
        building = sym BUILDING,
        not_found = sym NOT_FOUND,
        back = sym BACK,
    )
}

/// Verify mode: checks a marked list (`check(record, key type, key)`), then runs the scan.
///
/// # Safety
/// Only jumped to from the new-key path's scan.
#[unsafe(naked)]
unsafe extern "C" fn check_stub() {
    naked_asm!(
        "mov eax, dword ptr [esi + 0x0C]",
        "mov edi, dword ptr [ebp + 0x10]",
        "mov ecx, dword ptr [ebp + 8]",
        "cmp ecx, dword ptr [{building}]",
        "jne 2f",
        "pushad",
        "push edi",
        "push dword ptr [ebp + 0x0C]",
        "push esi",
        "call {check}",
        "add esp, 12",
        "popad",
        "2:",
        "jmp dword ptr [{back}]",
        building = sym BUILDING,
        check = sym check,
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(BUILDER, "icon_states builder")? + SITE;
    let set = core.call_target(site)?;
    core.require(set, SET_HEAD, "associative set")?;
    let list = core.jcc_at(set + 8, Cond::Equal)?.target;
    core.require(list, LIST, "associative set: list branch")?;
    let list_set = core.call_target(list + 15)?;
    core.require(list_set, LIST_SET, "list associative set")?;
    let new_key = core.call_target(list_set + 107)?;
    core.require(new_key, NEW_KEY, "new-key path")?;
    let (scan, not_found) = (new_key + SCAN, new_key + NOT_FOUND_AT);
    ensure!(core.rel8(new_key + EMPTY_JZ)? == not_found, "new-key path: unexpected not-found exit");

    SET.set(set);
    NOT_FOUND.set(not_found);
    BACK.set(scan + 6);
    let stub = if verify() { check_stub as *const () } else { skip_stub as *const () };
    // The scan stub first: it does nothing until the builder marks a list.
    core.jump(scan, 6, stub)?;
    core.call(site, 5, set_state as SetFn)?;
    Ok(core.report(&[site, new_key]))
}
