//! `procs`, Linux: libbyond's calls to its find-or-add redirected, its append entered by [`append`]. The function
//! itself stays as it is, so tools that find it by its bytes (auxtools' hot reload) still do; their calls go to it,
//! and the index rebuilds when it sees the count they changed.

use std::arch::naked_asm;

use super::{FindFn, Globals, find_or_add, init};
use crate::base::code::Addr;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The address of BYOND's count global.
static COUNT: Addr = Addr::new();
/// The original after its scan loop (`cmp eax,0FFFFFFh`): append with eax = count, edi = key.
static APPEND_AT: Addr = Addr::new();

/// The find-or-add: push edi ; push esi ; push ebx ; sub esp,10h ; mov eax,[count] ; mov edi,[esp+20h] ;
/// test eax,eax ; je ; xor esi,esi ; xor ebx,ebx ; jmp first ; (pad) ; next: mov eax,[count] ; add ebx,1 ;
/// add esi,2Ch ; cmp eax,ebx ; jbe append ; first: mov eax,[array] ; sub esp,8 ; push edi ; add eax,esi ; push eax ;
/// call compare ; add esp,10h ; test eax,eax ; jne next ; (found) add esp,10h ; mov eax,ebx ; pop ebx ; pop esi ;
/// pop edi ; ret ; append: cmp eax,0FFFFFFh
const FIND: &str = "57 56 53 83 EC 10 A1 ?? ?? ?? ?? 8B 7C 24 20 85 C0 0F 84 ?? ?? ?? ?? 31 F6 31 DB EB ?? ?? ?? ?? \
                    A1 ?? ?? ?? ?? 83 C3 01 83 C6 2C 39 D8 76 ?? A1 ?? ?? ?? ?? 83 EC 08 57 01 F0 50 E8 ?? ?? ?? ?? \
                    83 C4 10 85 C0 75 ?? 83 C4 10 89 D8 5B 5E 5F C3 3D FF FF FF 00";
/// The comparator (dword and byte differences in this order), which defines equality.
const COMPARE: &str = "53 8B 4C 24 08 8B 54 24 0C 8B 01 2B 02 75 ?? 8B 41 1C 2B 42 1C 75 ?? 8B 41 14 2B 42 14 75 ?? \
                       0F BE 41 1A 0F BE 5A 1A 29 D8 75 ?? 8B 41 04 2B 42 04 75 ?? 8B 41 08 2B 42 08 75 ?? \
                       8B 41 0C 2B 42 0C 75 ?? 0F BE 41 18 0F BE 5A 18 29 D8 75 ?? 0F B6 41 19 0F B6 5A 19 29 D8 75 ?? \
                       8B 41 20 2B 42 20 75 ?? 8B 41 24 2B 42 24 5B C3";

/// Enters the original after its scan loop, as if the scan had found nothing.
///
/// # Safety
/// As the original: `_key` is a live key record.
#[unsafe(naked)]
unsafe extern "C" fn append(_key: usize) -> u32 {
    naked_asm!(
        // The original's frame: push edi ; push esi ; push ebx ; sub esp,10h ; mov edi,[esp+20h]
        "push edi",
        "push esi",
        "push ebx",
        "sub esp, 0x10",
        "mov edi, dword ptr [esp + 0x20]",
        "mov eax, dword ptr [{count}]",
        "mov eax, dword ptr [eax]",
        "jmp dword ptr [{append}]",
        count = sym COUNT,
        append = sym APPEND_AT,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(FIND, "proc-table find-or-add")?;
    let (count, array) = (core.ptr_at(f + 7)?, core.ptr_at(f + 0x30)?);
    ensure!(core.ptr_at(f + 0x21)? == count, "proc-table find-or-add: count globals differ");
    let cmp = core.find_unique(COMPARE, "proc-table comparator")?;
    ensure!(core.call_target(f + 0x3B)? == cmp, "proc-table find-or-add: unexpected comparator");
    COUNT.set(count);
    APPEND_AT.set(f + 0x50);
    let calls = core.calls_to(f)?;
    ensure!(!calls.is_empty(), "proc-table find-or-add: no callers");
    for &c in calls.iter() {
        core.retarget(c + 1, find_or_add as FindFn)?;
    }
    init(Globals { count, array, original: f, append: append as FindFn as usize })?;
    Ok(core.report(calls.as_slice()))
}
