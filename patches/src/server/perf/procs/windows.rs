//! `procs`, Windows: byondcore's calls to its find-or-add redirected, its append entered by [`append`]. The function
//! itself stays as it is, so tools that find it by its bytes (auxtools' hot reload) still do; their calls go to it,
//! and the index rebuilds when it sees the count they changed.

use std::arch::naked_asm;

use super::{FindFn, Globals, find_or_add, init};
use crate::base::code::Addr;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The address of BYOND's count global.
static COUNT: Addr = Addr::new();
/// The original after its scan loop (`cmp eax,0FFFFFFh`): append with eax = count, ebx = key.
static APPEND_AT: Addr = Addr::new();

/// The find-or-add: push ebp ; mov ebp,esp ; mov eax,[count] ; push ebx ; mov ebx,[ebp+8] ; push esi ; xor esi,esi ;
/// push edi ; test eax,eax ; je append ; xor edi,edi ; loop: mov eax,[array] ; add eax,edi ; push ebx ; push eax ;
/// call compare ; add esp,8 ; test eax,eax ; je found ; mov eax,[count] ; inc esi ; add edi,2Ch ; cmp esi,eax ;
/// jb loop ; append: cmp eax,0FFFFFFh ; jne
const FIND: &str = "55 8B EC A1 ?? ?? ?? ?? 53 8B 5D 08 56 33 F6 57 85 C0 74 24 33 FF A1 ?? ?? ?? ?? 03 C7 53 50 E8 ?? ?? ?? ?? \
                    83 C4 08 85 C0 74 31 A1 ?? ?? ?? ?? 46 83 C7 2C 3B F0 72 DE 3D FF FF FF 00 75 24";
/// The comparator (dword and byte differences in this order), which defines equality.
const COMPARE: &str = "55 8B EC 8B 45 08 8B 55 0C 8B 08 2B 0A 74 ?? 8B C1 5D C3 8B 48 1C 2B 4A 1C 75 ?? 8B 48 14 2B 4A 14 \
                       75 ?? 0F BE 4A 1A 56 0F BE 70 1A 2B F1 75 ?? 8B 48 04 2B 4A 04 75 ?? 8B 48 08 2B 4A 08 75 ?? \
                       8B 48 0C 2B 4A 0C 75 ?? 0F BE 70 18 0F BE 4A 18 2B F1 75 ?? 0F B6 70 19 0F B6 4A 19 2B F1 74 ?? \
                       8B C6 5E 5D C3 8B 48 20 2B 4A 20 74 ?? 8B C1 5E 5D C3 8B 40 24 2B 42 24 5E 5D C3";

/// Enters the original after its scan loop, as if the scan had found nothing.
///
/// # Safety
/// As the original: `_key` is a live key record.
#[unsafe(naked)]
unsafe extern "C" fn append(_key: usize) -> u32 {
    naked_asm!(
        // The original's prologue: push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+8] ; push esi ; push edi
        "push ebp",
        "mov ebp, esp",
        "push ebx",
        "mov ebx, dword ptr [ebp + 8]",
        "push esi",
        "push edi",
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
    let (count, array) = (core.ptr_at(f + 4)?, core.ptr_at(f + 0x17)?);
    ensure!(core.ptr_at(f + 0x2C)? == count, "proc-table find-or-add: count globals differ");
    let cmp = core.find_unique(COMPARE, "proc-table comparator")?;
    ensure!(core.call_target(f + 0x1F)? == cmp, "proc-table find-or-add: unexpected comparator");
    COUNT.set(count);
    APPEND_AT.set(f + 0x38);
    let calls = core.calls_to(f)?;
    ensure!(!calls.is_empty(), "proc-table find-or-add: no callers");
    for &c in calls.iter() {
        core.retarget(c + 1, find_or_add as FindFn)?;
    }
    init(Globals { count, array, original: f, append: append as FindFn as usize })?;
    Ok(core.report(calls.as_slice()))
}
