//! `strtab`, Linux: the intern jumps to [`intern`], and the save is hooked by [`save_entry`]. Misses enter the
//! original past its walk with the tree link aimed at a dummy, so the entry is created but not linked.

use std::arch::naked_asm;
use std::sync::atomic::AtomicU32;

use super::{FOUND, Found, intern, relink};
use crate::base::code::{Addr, Foreign, hx};
use crate::base::feature::Ctx;
use crate::base::linux::symbol;
use crate::base::{Result, ensure};

/// Where the parts of one build range's intern are.
struct Layout {
    intern: &'static str,
    count_var: usize,
    table_var: usize,
    /// Past the walk, where the insert continues: it reloads the count and the table (`8B 1D count`, then the table's
    /// load at `after_table`).
    after_walk: usize,
    after_table: usize,
    insert: unsafe extern "C" fn(),
}

/// Before 516.1689: push ebp ; push edi ; push esi ; push ebx ; sub esp,1Ch ; mov ebx,[count] ; mov edx,[table] ;
/// mov esi,[esp+30h] (entry) ; ... the walk from slot 0: compare, left +8 / right +0Ch, ebp = &link ... ;
/// after the walk: mov ebx,[count] ; mov edx,[table]
const OLD: Layout = Layout {
    intern: "55 57 56 53 83 EC 1C 8B 1D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 8B 74 24 30 85 DB 0F 84 ?? ?? ?? ?? \
             8B 1A 85 DB 75 14 E9 ?? ?? ?? ?? 8D B6 00 00 00 00 8D 6B 0C 89 C3 85 DB 74 26 83 EC 08 53 56 \
             E8 ?? ?? ?? ?? 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? 8B 53 08 8B 43 0C 7F DA 8D 6B 08 89 D3 85 DB \
             75 DA 8B 1D ?? ?? ?? ?? 8B 15",
    count_var: 9,
    table_var: 15,
    after_walk: 0x5F,
    after_table: 8,
    insert: insert_old,
};

/// From 516.1689: push ebp ; push edi ; push esi ; push ebx ; sub esp,2Ch ; mov ebx,[count] ; mov eax,[table] ;
/// mov edi,[esp+40h] (entry) ; ... the walk from slot 0: compare, left +8 / right +0Ch, edx = &link, ebp = depth ... ;
/// after the walk: mov ebx,[count] ; mov eax,[table], then the next id (the count), the insert, and a rebalance of
/// the tree when the depth was over 99.
const NEW: Layout = Layout {
    intern: "55 57 56 53 83 EC 2C 8B 1D ?? ?? ?? ?? A1 ?? ?? ?? ?? 8B 7C 24 40 85 DB 74 ?? 8B 30 85 F6 \
             0F 84 ?? ?? ?? ?? 31 ED EB 11 8D B4 26 00 00 00 00 90 8D 56 0C 89 CE 85 F6 74 2B 83 EC 08 56 57 \
             E8 ?? ?? ?? ?? 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? 8B 5E 08 8B 4E 0C 83 C5 01 85 C0 7F D5 8D 56 08 \
             89 DE 85 F6 75 D5 8B 1D ?? ?? ?? ?? A1",
    count_var: 9,
    table_var: 14,
    after_walk: 0x64,
    after_table: 7,
    insert: insert_new,
};

/// Both: the compare call and the `je hit` after it.
const COMPARE_CALL: usize = 0x3E;
const JE_HIT: usize = 0x48;
/// The compare: strcmp of the entries' strings, whose order slices reproduce.
const COMPARE: &str = "8B 44 24 08 8B 00 89 44 24 08 8B 44 24 04 8B 00 89 44 24 04 E9";
const COMPARE_JMP: usize = 0x14;
/// The hit path: mov eax,[esp+keep] ; cmp byte [eax],0 ; je free ; ... free: sub esp,0Ch ; push [entry's string] ;
/// call free
const HIT_CODE: &str = "8B 44 24 ?? 80 38 00";
const HIT_FREE: &str = "83 EC 0C FF ?? E8";

/// The intern just past its walk.
static AFTER: Addr = Addr::new();
/// The save's original (from its hook).
static SAVE: Addr = Addr::new();
/// Where the insert stores the new entry instead of a tree link.
static DUMMY_LINK: AtomicU32 = AtomicU32::new(0);

/// The insert before 516.1689 (cdecl `(entry, keep) -> id`): the intern's prologue, then its code past the walk.
///
/// # Safety
/// Only called through [`Found::insert`].
#[unsafe(naked)]
unsafe extern "C" fn insert_old() {
    naked_asm!(
        "push ebp",
        "push edi",
        "push esi",
        "push ebx",
        "sub esp, 0x1c",
        "mov esi, dword ptr [esp + 0x30]",
        "lea ebp, [{dummy}]",
        "jmp dword ptr [{after}]",
        dummy = sym DUMMY_LINK,
        after = sym AFTER,
    )
}

/// The insert from 516.1689: as [`insert_old`] with the larger frame, the entry in edi, the link in edx, and depth 0.
///
/// # Safety
/// Only called through [`Found::insert`].
#[unsafe(naked)]
unsafe extern "C" fn insert_new() {
    naked_asm!(
        "push ebp",
        "push edi",
        "push esi",
        "push ebx",
        "sub esp, 0x2c",
        "mov edi, dword ptr [esp + 0x40]",
        "lea edx, [{dummy}]",
        "xor ebp, ebp",
        "jmp dword ptr [{after}]",
        dummy = sym DUMMY_LINK,
        after = sym AFTER,
    )
}

/// The save's hook: relink, then the save.
///
/// # Safety
/// Only jumped to from the save's entry.
#[unsafe(naked)]
unsafe extern "C" fn save_entry() {
    naked_asm!(
        "pushad",
        "mov ebp, esp",
        "and esp, -16",
        "call {relink}",
        "mov esp, ebp",
        "popad",
        "jmp dword ptr [{save}]",
        relink = sym relink,
        save = sym SAVE,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let l = if core.probe_all(NEW.intern)?.is_empty() { &OLD } else { &NEW };
    let f = core.find_unique(l.intern, "string intern")?;
    let (count, table) = (core.ptr_at(f + l.count_var)?, core.ptr_at(f + l.table_var)?);
    let after = f + l.after_walk;
    ensure!(
        core.ptr_at(after + 2)? == count && core.ptr_at(after + l.after_table)? == table,
        "string intern: walk exit does not reload the table"
    );
    let compare = core.follow_call(f + COMPARE_CALL, COMPARE, "string intern: compare")?;
    let strcmp = symbol(c"strcmp")?;
    ensure!(core.rel32(compare + COMPARE_JMP + 1)? == strcmp, "string intern: compare isn't strcmp");
    let hit = core.branch_at(f + JE_HIT)?.target;
    core.require(hit, HIT_CODE, "string intern: hit path")?;
    let free_path = core.branch_at(hit + HIT_CODE.split_whitespace().count())?.target;
    core.require(free_path, HIT_FREE, "string intern: hit path's free")?;
    let free = symbol(c"free")?;
    ensure!(core.call_target(free_path + 5)? == free, "string intern: hit path does not free the entry's string");
    let save = core.find_unique(
        &format!(
            "55 57 56 53 83 EC 48 8D 44 24 1C 50 E8 ?? ?? ?? ?? 8B 3D {} 83 C4 10 85 FF 74 17 A1 {}",
            hx(count),
            hx(table)
        ),
        "string-table save",
    )?;

    AFTER.set(after);
    SAVE.set(core.hook(save, 7, save_entry as unsafe extern "C" fn())?);
    core.jump(f, 7, intern as unsafe extern "C" fn(usize, *const u8) -> u32)?;
    let insert = Foreign::at(l.insert);
    FOUND.commit(cx, Found { table, count, free: Foreign::at(free), insert });
    Ok(format!("{}, save relink {}", core.report(&[f]), core.report(&[save])))
}
