//! `protos`, Windows: the tree's lookup-or-insert (thiscall) jumps at entry to [`tree_stub`].

use std::arch::naked_asm;

use super::{lookup, record};
use crate::base::Result;
use crate::base::code::Addr;
use crate::base::feature::Ctx;

/// BYOND's lookup-or-insert (its moved entry, then the rest): thiscall, ecx = tree, holder and `*inserted` on the
/// stack.
static ORIGINAL: Addr = Addr::new();

/// The lookup-or-insert: push ebp ; mov ebp,esp ; sub esp,0Ch ; push ebx ; mov eax,ecx ; xor ebx,ebx ; push esi ;
/// push edi ; mov [ebp-4],eax ; mov edi,[eax]
const TREE: &str = "55 8B EC 83 EC 0C 53 8B C1 33 DB 56 57 89 45 FC 8B 38";
/// The hooked bytes: push ebp ; mov ebp,esp ; sub esp,0Ch.
const ENTRY: usize = 6;

/// Replaces the lookup-or-insert (thiscall: ecx = tree, [esp+4] = key holder, [esp+8] = `*inserted`; returns the
/// node, pops 8). BYOND's runs from asm, so no Rust frame is on the stack if it throws.
///
/// # Safety
/// Only jumped to from the lookup-or-insert's entry.
#[unsafe(naked)]
unsafe extern "C" fn tree_stub() {
    naked_asm!(
        "push ecx",
        "push dword ptr [esp + 8]",
        "push ecx",
        "call {lookup}",
        "add esp, 8",
        "pop ecx",
        "test eax, eax",
        "jz 2f",
        "mov edx, dword ptr [esp + 8]",
        "mov byte ptr [edx], 0",
        "ret 8",
        "2:",
        // [esp] = ret, [esp+4] = holder, [esp+8] = flag
        "push ecx",
        "push dword ptr [esp + 12]",
        "push dword ptr [esp + 12]",
        "call dword ptr [{original}]",
        "pop ecx",
        // [esp] = ret, [esp+4] = holder
        "push eax",
        "push eax",
        "push dword ptr [esp + 12]",
        "push ecx",
        "call {record}",
        "add esp, 12",
        "pop eax",
        "ret 8",
        lookup = sym lookup,
        original = sym ORIGINAL,
        record = sym record,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(TREE, "proto tree")?;
    ORIGINAL.set(core.hook(f, ENTRY, tree_stub as unsafe extern "C" fn())?);
    Ok(core.report(&[f]))
}
