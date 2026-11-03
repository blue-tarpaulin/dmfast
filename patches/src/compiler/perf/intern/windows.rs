//! `internidx` and `rebuild`, Windows: byondcore's intern (thiscall) jumps at entry to [`intern_stub`], and each of
//! its inlined rebuild checks to a stub asking [`should_rebuild`].

use std::arch::naked_asm;

use super::{COUNT, init_index, init_rebuild, lookup, record, should_rebuild};
use crate::base::code::{Addr, Cond, Stub, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The intern: SEH prologue, then mov [ebp-10h],ecx ; mov esi,[ecx+80h] ; lea eax,[ecx+80h] ; mov ecx,[ebp+8] ;
/// mov ebx,eax ; mov [ebp-14h],eax ; test esi,esi ; je
const INTERN: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 1C 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 \
                      8D 45 F4 64 A3 00 00 00 00 89 4D F0 8B B1 80 00 00 00 8D 81 80 00 00 00 8B 4D 08 8B D8 \
                      89 45 EC 85 F6 74";
/// The moved entry: push ebp ; mov ebp,esp ; push -1 ; push handler.
const ENTRY: usize = 10;

/// After an insert: inc dword [ebx+88h] ; cmp byte [ebx+88h],0 ; jne skip ; xor ebx,ebx (the rebuild). Inlined in
/// the intern and in the keyword table's setup.
const CHECK: &str = "FF 83 88 00 00 00 80 BB 88 00 00 00 00 0F 85 ?? ?? ?? ?? 33 DB";
/// The replaced `cmp ; jne`, followed by the rebuild.
const CMP: usize = 6;
const JNE: usize = 13;
const CHECK_END: usize = 19;

/// BYOND's intern: its moved entry, then the rest.
static ORIGINAL: Addr = Addr::new();

/// Replaces the intern (thiscall: ecx = table, [esp+4] = string; returns the id, pops 4). BYOND's runs from asm, so
/// no Rust frame is on the stack if it throws.
///
/// # Safety
/// Only jumped to from the intern's entry.
#[unsafe(naked)]
unsafe extern "C" fn intern_stub() {
    naked_asm!(
        "push ecx",
        "push dword ptr [esp + 8]",
        "push ecx",
        "call {lookup}",
        "add esp, 8",
        "pop ecx",
        "test eax, eax",
        "jz 2f",
        "ret 4",
        "2:",
        // [esp] = ret, [esp+4] = string
        "push ecx",
        "push dword ptr [esp + 8]",
        "call dword ptr [{original}]",
        "pop ecx",
        // [esp] = ret, [esp+4] = string
        "push eax",
        "push eax",
        "push dword ptr [esp + 12]",
        "push ecx",
        "call {record}",
        "add esp, 12",
        "pop eax",
        "ret 4",
        lookup = sym lookup,
        original = sym ORIGINAL,
        record = sym record,
    )
}

pub(super) fn install_index(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(INTERN, "identifier intern")?;
    // Not `Image::hook`: the moved `push handler` is an absolute address with a relocation (copied as loaded).
    ORIGINAL.set(trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?);
    init_index()?;
    core.jump(f, ENTRY, intern_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[f]))
}

/// Replaces `cmp byte [ebx+88h],0 ; jne skip` (ebx = table) keeping every register: rebuild (fall through to
/// `rebuild`) when [`should_rebuild`] says so, else `skip`.
fn rebuild_stub(skip: usize, rebuild: usize) -> Result<usize> {
    Stub::new()
        .bytes(&[0x50]) // push eax
        .bytes(&[0x51]) // push ecx
        .bytes(&[0x52]) // push edx
        .bytes_u32(&[0xFF, 0xB3], COUNT as u32, &[]) // push dword [ebx+88h]
        .call(should_rebuild as extern "C" fn(u32) -> u32)
        .bytes(&[0x83, 0xC4, 0x04]) // add esp,4
        .bytes(&[0x85, 0xC0]) // test eax,eax
        .bytes(&[0x5A]) // pop edx
        .bytes(&[0x59]) // pop ecx
        .bytes(&[0x58]) // pop eax (pops keep the flags)
        .jcc(Cond::Zero, skip)
        .jmp(rebuild)
        .exec()
}

pub(super) fn install_rebuild(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.find_all(CHECK)?;
    ensure!((1..=2).contains(&sites.len()), "intern rebuild check: expected 1-2, found {}", sites.len());
    let k = init_rebuild(cx);
    for &m in &sites {
        let skip = core.jcc_at(m + JNE, Cond::NotEqual)?.target;
        core.jump(m + CMP, CHECK_END - CMP, rebuild_stub(skip, m + CHECK_END)?)?;
    }
    Ok(format!("{}, K={k}", core.report(sites.as_slice())))
}
