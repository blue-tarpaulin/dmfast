//! `iconstates`, Linux: the builder's three calls of the set go to [`set_state`]; the stub replaces the scan's first
//! two instructions (`mov edx,[ebp+0Ch] ; lea eax,[edx-1]`).

use std::arch::naked_asm;

use super::verify::check;
use super::{BUILDING, SET, SetFn, set_state};
use crate::base::code::Addr;
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The builder: push 0 ; call new_list ; lea edi,[esp+28h] ; push 0Fh ; push eax ; push edi ; call value (the list,
/// at [esp+28h]) ; ... ; the all-states loop: push -1 ; push [state+14h] ; call text ; then [`SITE`]. Nothing else in
/// its loops touches the list.
const BUILDER: &str = "83 EC 0C 6A 00 E8 ?? ?? ?? ?? 8D 7C 24 28 83 C4 0C 6A 0F 50 57 E8 ?? ?? ?? ?? F3 0F 7E 44 24 24 0F \
     29 44 24 0C 83 C4 0C 80 7D 04 00 0F 84 ?? ?? ?? ?? 8B 75 0C 85 F6 7E ?? 31 F6 8D 74 26 00 90 83 EC 08 6A FF 8B 45 \
     08 8B 04 B0 83 C6 01 FF 70 14 E8 ?? ?? ?? ?? 83 C4 0C 6A 06 50 57 E8 ?? ?? ?? ?? 59 FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? \
     ?? FF 74 24 2C FF 74 24 2C FF 74 24 1C FF 74 24 1C E8";
/// Its first call of the set.
const FIRST: usize = 127;
/// A call of the set after making a state's name a value: call value ; pop reg ; `push [null]` x2 ; push the name ;
/// push the list ; call set (at [`SITE_CALL`]). Its other two loops (movement states only, the others) end in it too.
const SITE: &str = "E8 ?? ?? ?? ?? 83 C4 0C 6A 06 50 57 E8 ?? ?? ?? ?? ?? FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? FF 74 24 2C \
                    FF 74 24 2C FF 74 24 1C FF 74 24 1C E8";
const SITE_CALL: usize = 46;
/// How far into the builder its three calls are.
const SPAN: usize = 0x260;
/// The set: push ebx ; sub esp,38h ; (load the arguments) ; cmp al,0Fh ; je list ; cmp al,55h ; je alist
/// (516.1660 on), or jz near list ; jne error (before).
const SET_HEAD: &str = "53 83 EC 38 F3 0F 7E 44 24 48 8B 44 24 40 8B 54 24 44 0F 29 04 24 F3 0F 7E 44 24 50 3C 0F";
const LIST_JE: usize = 0x1E;
/// The list branch: re-push the arguments but the type and jump to the list set.
const LIST: &str = "F3 0F 7E 14 24 89 54 24 40 66 0F D6 44 24 4C 66 0F D6 54 24 44 83 C4 38 5B E9";
/// The list set: look the key up in the tree ([record+4]); found: replace the value; else jump to the new-key path
/// with eax = the list id and edx:ecx = the key.
const LIST_SET: &str = "57 56 53 83 EC 2C F3 0F 7E 44 24 40 8B 74 24 3C 0F 29 44 24 0C 66 0F 7E C7 66 0F 73 D0 20 66 0F \
     7E C3 F3 0F 7E 44 24 48 0F 29 44 24 1C 56 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? 83 EC 04 FF 74 24 08 FF 74 24 08 FF 70 \
     04 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? F3 0F 7E 40 08 F3 0F 7E 4C 24 10 66 0F D6 48 08 66 0F D6 44 24 30 83 C4 20 5B \
     5E 5F E9 ?? ?? ?? ?? 8D 76 00 F3 0F 7E 54 24 10 89 FA 89 D9 89 F0 66 0F D6 54 24 30 83 C4 20 5B 5E 5F E9";
const NEW_KEY_JMP: usize = 144;
/// The new-key path: edi = the id, bl/esi = the key, ebp = the record ; scan the items from the end ; not found: make
/// the list a value, append the key, then insert it into the tree ([ebp+4]).
const NEW_KEY: &str = "55 66 0F 6E C2 66 0F 6E C9 57 66 0F 62 C1 89 C7 56 53 66 0F 7E C3 83 EC 48 0F 29 44 24 0C 66 0F \
     73 D0 20 66 0F 7E C6 F3 0F 7E 44 24 5C 0F 29 44 24 1C 50 E8 ?? ?? ?? ?? 83 C4 10 89 C5 85 C0 74 ?? 8B 55 0C 8D 42 FF \
     85 D2 74 ?? 8B 4D 00 EB 09 8D 50 FF 85 C0 74 ?? 89 D0 3A 1C C1 75 F2 3B 74 C1 04 75 EC 85 C0 78 ?? 83 EC 04 83 C5 04 \
     FF 74 24 08 FF 74 24 08 55 E8 ?? ?? ?? ?? F3 0F 7E 54 24 20 66 0F D6 50 08 83 C4 4C 5B 5E 5F 5D C3";
/// In the new-key path: the scan, and its three ways out without a match (empty list, start reached, index < 0:
/// rel8 fields).
const SCAN: usize = 0x41;
const NOT_FOUND_JCC: [usize; 3] = [0x4A, 0x56, 0x67];
/// The not-found branch: make the list a value.
const NOT_FOUND_CODE: &str = "8D 44 24 28 83 EC 04 6A 0F 57 50 E8";

/// The scan's not-found branch and the instruction after the displaced ones.
static NOT_FOUND: Addr = Addr::new();
static BACK: Addr = Addr::new();

/// # Safety
/// Only jumped to from the new-key path's scan.
#[unsafe(naked)]
unsafe extern "C" fn skip_stub() {
    naked_asm!(
        "cmp edi, dword ptr [{building}]",
        "jne 2f",
        "jmp dword ptr [{not_found}]",
        "2:",
        "mov edx, dword ptr [ebp + 0x0C]",
        "lea eax, [edx - 1]",
        "jmp dword ptr [{back}]",
        building = sym BUILDING,
        not_found = sym NOT_FOUND,
        back = sym BACK,
    )
}

/// Verify mode: `check(record, key type, key)` on a marked list (16-byte aligned call), then the scan.
///
/// # Safety
/// Only jumped to from the new-key path's scan.
#[unsafe(naked)]
unsafe extern "C" fn check_stub() {
    naked_asm!(
        "cmp edi, dword ptr [{building}]",
        "jne 2f",
        "pushad",
        "mov eax, esp",
        "and esp, -16",
        "push eax",
        "push esi",
        "push ebx",
        "push ebp",
        "call {check}",
        "add esp, 12",
        "pop esp",
        "popad",
        "2:",
        "mov edx, dword ptr [ebp + 0x0C]",
        "lea eax, [edx - 1]",
        "jmp dword ptr [{back}]",
        building = sym BUILDING,
        check = sym check,
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let builder = core.find_unique(BUILDER, "icon_states builder")?;
    let first = builder + FIRST;
    let sites: Vec<usize> = core.find_in(builder, SPAN, SITE)?.iter().map(|&m| m + SITE_CALL).collect();
    ensure!(
        sites.len() == 3 && sites.contains(&first),
        "icon_states builder: {} calls of the set, expected 3",
        sites.len()
    );
    let set = core.call_target(first)?;
    for &s in &sites {
        ensure!(core.call_target(s)? == set, "icon_states builder: its calls go to different functions");
    }
    let list = if core.is_at(set, &format!("{SET_HEAD} 74 ?? 3C 55 74 ??"))? {
        core.rel8(set + LIST_JE + 1)?
    } else {
        core.require(set, &format!("{SET_HEAD} 0F 84 ?? ?? ?? ?? 3C 55 75 ??"), "associative set")?;
        core.rel32(set + LIST_JE + 2)?
    };
    core.require(list, LIST, "associative set: list branch")?;
    let list_set = core.rel32(list + 26)?;
    core.require(list_set, LIST_SET, "list associative set")?;
    let new_key = core.rel32(list_set + NEW_KEY_JMP + 1)?;
    core.require(new_key, NEW_KEY, "new-key path")?;
    let not_found = core.rel8(new_key + NOT_FOUND_JCC[0])?;
    ensure!(
        NOT_FOUND_JCC.iter().all(|&j| core.rel8(new_key + j).is_ok_and(|t| t == not_found))
            && core.is_at(not_found, NOT_FOUND_CODE)?,
        "new-key path: unexpected not-found branch"
    );

    let scan = new_key + SCAN;
    SET.set(set);
    NOT_FOUND.set(not_found);
    BACK.set(scan + 6);
    let stub = if verify() { check_stub as *const () } else { skip_stub as *const () };
    // The scan stub first: it does nothing until the builder marks a list.
    core.jump(scan, 6, stub)?;
    for &s in &sites {
        core.call(s, 5, set_state as SetFn)?;
    }
    Ok(core.report(&[first, new_key]))
}
