//! `strtab`, Linux: the intern jumps to [`intern`], and the save is hooked by [`save_entry`]. Misses enter the
//! original past its walk with the tree link (ebp) aimed at a dummy, so the entry is created but not linked.

use std::arch::naked_asm;
use std::sync::atomic::AtomicU32;

use super::{FOUND, Found, intern, relink};
use crate::base::code::{Addr, Foreign, hx};
use crate::base::feature::Ctx;
use crate::base::linux::symbol;
use crate::base::{Result, ensure};

/// The intern: push ebp ; push edi ; push esi ; push ebx ; sub esp,1Ch ; mov ebx,[count] ; mov edx,[table] ;
/// mov esi,[esp+30h] (entry) ; ... the walk from slot 0: compare, left +8 / right +0Ch, ebp = &link ... ;
/// after the walk: mov ebx,[count] ; mov edx,[table]
const INTERN: &str = "55 57 56 53 83 EC 1C 8B 1D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 8B 74 24 30 85 DB 0F 84 ?? ?? ?? ?? \
                      8B 1A 85 DB 75 14 E9 ?? ?? ?? ?? 8D B6 00 00 00 00 8D 6B 0C 89 C3 85 DB 74 26 83 EC 08 53 56 \
                      E8 ?? ?? ?? ?? 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? 8B 53 08 8B 43 0C 7F DA 8D 6B 08 89 D3 85 DB \
                      75 DA 8B 1D ?? ?? ?? ?? 8B 15";
const COUNT_VAR: usize = 9;
const TABLE_VAR: usize = 15;
/// The 7 bytes replaced: the prologue [`insert_entry`] repeats.
const ENTRY: usize = 7;
const COMPARE_CALL: usize = 0x3E;
/// The compare: strcmp of the entries' strings, whose order slices reproduce.
const COMPARE: &str = "8B 44 24 08 8B 00 89 44 24 08 8B 44 24 04 8B 00 89 44 24 04 E9";
const COMPARE_JMP: usize = 0x14;
/// `je hit` after the compare.
const JE_HIT: usize = 0x48;
const AFTER_WALK: usize = 0x5F;
/// The hit path: mov eax,[esp+34h] ; cmp byte [eax],0 ; je free ; ... free: sub esp,0Ch ; push [esi] ; call free
const HIT_CODE: &str = "8B 44 24 34 80 38 00 74";
const HIT_FREE: &str = "83 EC 0C FF 36 E8";
/// The save: push ebp ; push edi ; push esi ; push ebx ; sub esp,48h ; ... ; mov edi,[count] ; ... ; mov eax,[table]
const SAVE_ENTRY: usize = 7;

/// The intern just past its walk.
static AFTER: Addr = Addr::new();
/// The save's original (from its hook).
static SAVE: Addr = Addr::new();
/// Where the insert stores the new entry instead of a tree link.
static DUMMY_LINK: AtomicU32 = AtomicU32::new(0);

/// The insert (cdecl `(entry, keep) -> id`): the intern's prologue, then its code past the walk, which reloads the
/// count and the table itself.
///
/// # Safety
/// Only called through [`Found::insert`].
#[unsafe(naked)]
unsafe extern "C" fn insert_entry() {
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
    let f = core.find_unique(INTERN, "string intern")?;
    let (count, table) = (core.ptr_at(f + COUNT_VAR)?, core.ptr_at(f + TABLE_VAR)?);
    let after = f + AFTER_WALK;
    ensure!(
        core.ptr_at(after + 2)? == count && core.ptr_at(after + 8)? == table,
        "string intern: walk exit does not reload the table"
    );
    let compare = core.follow_call(f + COMPARE_CALL, COMPARE, "string intern: compare")?;
    let strcmp = symbol(c"strcmp");
    ensure!(strcmp != 0 && core.rel32(compare + COMPARE_JMP + 1)? == strcmp, "string intern: compare isn't strcmp");
    let hit = core.rel32(f + JE_HIT + 2)?;
    core.require(hit, HIT_CODE, "string intern: hit path")?;
    let free_path = core.rel8(hit + HIT_CODE.split_whitespace().count())?;
    core.require(free_path, HIT_FREE, "string intern: hit path's free")?;
    let free = symbol(c"free");
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
    SAVE.set(core.hook(save, SAVE_ENTRY, save_entry as unsafe extern "C" fn())?);
    core.jump(f, ENTRY, intern as unsafe extern "C" fn(usize, *const u8) -> u32)?;
    let insert = Foreign::at(insert_entry as unsafe extern "C" fn());
    FOUND.commit(cx, Found { table, count, free: Foreign::at(free), insert });
    Ok(format!("{}, save relink {}", core.report(&[f]), core.report(&[save])))
}
