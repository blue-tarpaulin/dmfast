//! `strtab`, Windows: the intern jumps to [`intern`], and the save's entry to [`save_entry`], which continues into what
//! was there (`ftell`'s hook or the prologue).

use std::arch::naked_asm;

use super::{FOUND, Found, intern, relink};
use crate::base::code::{Addr, Foreign, Stub, branch, hx, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::string_table;

/// Where the parts of one build range's intern are.
struct Layout {
    intern: &'static str,
    table_var: usize,
    count_var: usize,
    /// What the insert runs of the intern: up to `mov dword [ebp-4],0` (the tree link it stores the new entry at:
    /// none; from 516.1689 also the walk depth in edi: 0, below the rebalance).
    prologue: usize,
    compare_call: usize,
    /// Past the walk, where the insert continues: it reloads the count (and, before 516.1689, the table).
    after_walk: usize,
    after_code: fn(count: usize, table: usize) -> String,
    hit: usize,
}

/// Before 516.1689: push ebp ; mov ebp,esp ; push ecx ; mov ecx,[table] ; push ebx ; mov ebx,[ebp+8] ; push esi ;
/// push edi ; mov edi,[count] ; mov dword [ebp-4],0 ; test edi,edi ; je ; mov esi,[ecx] ; test esi,esi ; je ;
/// walk: push esi ; push ebx ; mov edi,esi ; call compare. Past the walk: mov edi,[count] ; mov ecx,[table], then
/// the free-slot search.
const OLD: Layout = Layout {
    intern: "55 8B EC 51 8B 0D ?? ?? ?? ?? 53 8B 5D 08 56 57 8B 3D ?? ?? ?? ?? C7 45 FC 00 00 00 00 \
             85 FF 74 ?? 8B 31 85 F6 74 ?? 56 53 8B FE E8",
    table_var: 6,
    count_var: 18,
    prologue: 0x1D,
    compare_call: 0x2B,
    after_walk: 0x54,
    after_code: |count, table| format!("8B 3D {} 8B 0D {}", hx(count), hx(table)),
    hit: 0x7B,
};

/// From 516.1689: push ebp ; mov ebp,esp ; sub esp,8 ; mov ecx,[count] ; push ebx ; mov ebx,[ebp+8] ; push esi ;
/// push edi ; xor edi,edi (the depth) ; mov dword [ebp-4],0 ; mov [ebp-8],ecx ; test ecx,ecx ; je ;
/// mov eax,[table] ; mov esi,[eax] ; test esi,esi ; je ; walk: push esi ; push ebx ; mov [ebp-8],esi ;
/// call compare. Past the walk: mov ecx,[count] ; mov [ebp-8],ecx, then the next id (the count) and the insert.
const NEW: Layout = Layout {
    intern: "55 8B EC 83 EC 08 8B 0D ?? ?? ?? ?? 53 8B 5D 08 56 57 33 FF C7 45 FC 00 00 00 00 89 4D F8 \
             85 C9 74 ?? A1 ?? ?? ?? ?? 8B 30 85 F6 74 ?? 0F 1F 00 56 53 89 75 F8 E8",
    table_var: 0x23,
    count_var: 8,
    prologue: 0x1B,
    compare_call: 0x35,
    after_walk: 0x66,
    after_code: |count, _| format!("8B 0D {} 89 4D F8", hx(count)),
    hit: 0xC5,
};

/// The compare: unsigned bytes up to the first difference or NUL, as slices compare.
const COMPARE: &str = "55 8B EC 8B 45 0C 8B 08 8B 45 08 8B 00 0F 1F 00 8A 10 3A 11 75 1A 84 D2 74 12 8A 50 01 3A 51 01 \
                       75 0E 83 C0 02 83 C1 02 84 D2 75 E4 33 C0 5D C3 1B C0 83 C8 01 5D C3";
/// The hit path: mov eax,[ebp+0Ch] ; cmp byte [eax],0 ; jne ; push [ebx] ; call [free]
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
    let l = if core.probe_all(NEW.intern)?.is_empty() { &OLD } else { &NEW };
    let f = core.find_unique(l.intern, "string intern")?;
    let (table, count) = (core.ptr_at(f + l.table_var)?, core.ptr_at(f + l.count_var)?);
    ensure!(table == string_table::free_store(cx)?.table, "string intern: not the saved table");
    core.follow_call(f + l.compare_call, COMPARE, "string intern: compare")?;
    let after = f + l.after_walk;
    core.require(after, &(l.after_code)(count, table), "string intern: walk exit")?;
    core.require(f + l.hit, HIT_CODE, "string intern: hit path")?;
    let free = core.ptr_at(core.ptr_at(f + l.hit + HIT_CODE.split_whitespace().count())?)?;

    let save = string_table::save(cx)?;
    // Continue into ftell's hook if it installed (its `jmp`, live), else the original prologue.
    let back = match core.live_bytes(save, 1)?[0] {
        0xE9 => (save + 5).wrapping_add(core.live_u32(save + 1)? as i32 as usize),
        _ => trampoline(core.bytes(save, SAVE_PROLOGUE)?, save + SAVE_PROLOGUE)?,
    };
    let insert = Stub::new().bytes(core.bytes(f, l.prologue)?).jmp(after).exec()?;

    SAVE_BACK.set(back);
    core.wrap_code(save, &branch(0xE9, save, save_entry as unsafe extern "C" fn(), 5))?;
    core.jump(f, 5, intern as unsafe extern "C" fn(usize, *const u8) -> u32)?;
    FOUND.commit(cx, Found { table, count, free: Foreign::at(free), insert: Foreign::at(insert) });
    Ok(format!("{}, save relink {}", core.report(&[f]), core.report(&[save])))
}
