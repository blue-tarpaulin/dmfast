//! `strtab`, Windows: the intern jumps to [`intern`], and the save's entry to [`save_entry`], which continues into what
//! was there (`ftell`'s hook or the prologue).

use std::arch::naked_asm;

use super::{FOUND, Found, intern, relink};
use crate::base::code::{Addr, Foreign, Stub, branch, hx, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::string_table;

/// The intern: push ebp ; mov ebp,esp ; push ecx ; mov ecx,[table] ; push ebx ; mov ebx,[ebp+8] ; push esi ;
/// push edi ; mov edi,[count] ; mov dword [ebp-4],0 ; test edi,edi ; je ; mov esi,[ecx] ; test esi,esi ; je ;
/// walk: push esi ; push ebx ; mov edi,esi ; call compare
const INTERN: &str = "55 8B EC 51 8B 0D ?? ?? ?? ?? 53 8B 5D 08 56 57 8B 3D ?? ?? ?? ?? C7 45 FC 00 00 00 00 \
                      85 FF 74 ?? 8B 31 85 F6 74 ?? 56 53 8B FE E8";
const TABLE_VAR: usize = 6;
const COUNT_VAR: usize = 18;
/// What the insert runs of the intern: up to `mov dword [ebp-4],0` (the tree link it stores the new entry at: none).
const PROLOGUE: usize = 0x1D;
const COMPARE_CALL: usize = 0x2B;
/// The compare: unsigned bytes up to the first difference or NUL, as slices compare.
const COMPARE: &str = "55 8B EC 8B 45 0C 8B 08 8B 45 08 8B 00 0F 1F 00 8A 10 3A 11 75 1A 84 D2 74 12 8A 50 01 3A 51 01 \
                       75 0E 83 C0 02 83 C1 02 84 D2 75 E4 33 C0 5D C3 1B C0 83 C8 01 5D C3";
/// Past the walk: mov edi,[count] ; mov ecx,[table], then the free-slot search.
const AFTER_WALK: usize = 0x54;
/// The hit path: mov eax,[ebp+0Ch] ; cmp byte [eax],0 ; jne ; push [ebx] ; call [free]
const HIT: usize = 0x7B;
const HIT_CODE: &str = "8B 45 0C 80 38 00 75 11 FF 33 FF 15";
/// The save's prologue: push ebp ; mov ebp,esp ; sub esp,imm8.
const SAVE_PROLOGUE: usize = 6;

/// What was at the save's entry.
static SAVE_BACK: Addr = Addr::new();

/// The save's entry: relink, then continue into what was there.
///
/// # Safety
/// Only jumped to from the save's entry.
#[unsafe(naked)]
unsafe extern "C" fn save_entry() {
    naked_asm!(
        "pushad",
        "call {relink}",
        "popad",
        "jmp dword ptr [{back}]",
        relink = sym relink,
        back = sym SAVE_BACK,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(INTERN, "string intern")?;
    let (table, count) = (core.ptr_at(f + TABLE_VAR)?, core.ptr_at(f + COUNT_VAR)?);
    ensure!(table == string_table::free_store(cx)?.table, "string intern: not the saved table");
    core.follow_call(f + COMPARE_CALL, COMPARE, "string intern: compare")?;
    let after = f + AFTER_WALK;
    core.require(after, &format!("8B 3D {} 8B 0D {}", hx(count), hx(table)), "string intern: walk exit")?;
    core.require(f + HIT, HIT_CODE, "string intern: hit path")?;
    let free = core.ptr_at(core.ptr_at(f + HIT + HIT_CODE.split_whitespace().count())?)?;

    let save = string_table::save(cx)?;
    // Continue into ftell's hook if it installed (its `jmp`, live), else the original prologue.
    let back = match core.live_bytes(save, 1)?[0] {
        0xE9 => (save + 5).wrapping_add(core.live_u32(save + 1)? as i32 as usize),
        _ => trampoline(core.bytes(save, SAVE_PROLOGUE)?, save + SAVE_PROLOGUE)?,
    };
    let insert = Stub::new().bytes(core.bytes(f, PROLOGUE)?).jmp(after).exec()?;

    SAVE_BACK.set(back);
    core.wrap_code(save, &branch(0xE9, save, save_entry as unsafe extern "C" fn(), 5))?;
    core.jump(f, 5, intern as unsafe extern "C" fn(usize, *const u8) -> u32)?;
    FOUND.commit(cx, Found { table, count, free: Foreign::at(free), insert: Foreign::at(insert) });
    Ok(format!("{}, save relink {}", core.report(&[f]), core.report(&[save])))
}
