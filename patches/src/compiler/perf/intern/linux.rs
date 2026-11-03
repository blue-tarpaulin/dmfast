//! `internidx` and `rebuild`, Linux: libbyond's intern (cdecl) jumps at entry to [`intern_stub`], and its rebuild
//! check to [`rebuild_stub`].

use std::arch::naked_asm;

use super::{COUNT, init_index, init_rebuild, lookup, record, should_rebuild};
use crate::base::Result;
use crate::base::code::{Addr, Cond};
use crate::base::feature::Ctx;
use crate::base::image::Image;

/// The intern after its entry: eax = table, esi = string ; ebx = root (+80h) ; ebp = &root ; root empty -> insert.
/// `rebuild` finds it by this, as `internidx` rewrites the entry.
const BODY: &str = "8B 44 24 70 8B 74 24 74 8B 98 80 00 00 00 8D A8 80 00 00 00 89 6C 24 20 85 DB 75 1A";
/// The entry `internidx` moves: push ebp ; push edi ; push esi ; push ebx ; sub esp,5Ch.
const ENTRY: &str = "55 57 56 53 83 EC 5C";
const ENTRY_LEN: usize = 7;
/// How far the intern reaches, for the rebuild check.
const SPAN: usize = 0x200;

/// After an insert: mov eax,[edi+88h] ; test al,al ; jne skip ; mov esi,[edi+80h] (the rebuild).
const CHECK: &str = "8B 87 88 00 00 00 84 C0 0F 85 ?? ?? ?? ?? 8B B7 80 00 00 00";
const JNE: usize = 8;
/// The replaced `mov ; test ; jne`, followed by the rebuild.
const CHECK_LEN: usize = 14;
/// The skip path: add esp,5Ch ; pop ebx ; ... ; ret (eax = count).
const SKIP: &str = "83 C4 5C";

/// BYOND's intern: its moved entry, then the rest.
static ORIGINAL: Addr = Addr::new();
/// [`rebuild_stub`]'s exits: the rebuild, and the skip path.
static REBUILD_AT: Addr = Addr::new();
static SKIP_AT: Addr = Addr::new();

/// Replaces the intern (cdecl `(table, string) -> id`). BYOND's runs from asm, so no Rust frame is on the stack if it
/// throws.
///
/// # Safety
/// Only jumped to from the intern's entry.
#[unsafe(naked)]
unsafe extern "C" fn intern_stub() {
    naked_asm!(
        "push ebp",
        "mov ebp, esp",
        "and esp, -16",
        // [esp] table, [esp+4] string, [esp+8] id, [esp+12] the id kept across `record`
        "sub esp, 16",
        "mov eax, dword ptr [ebp + 8]",
        "mov dword ptr [esp], eax",
        "mov eax, dword ptr [ebp + 12]",
        "mov dword ptr [esp + 4], eax",
        "call {lookup}",
        "test eax, eax",
        "jnz 2f",
        "mov eax, dword ptr [ebp + 8]",
        "mov dword ptr [esp], eax",
        "mov eax, dword ptr [ebp + 12]",
        "mov dword ptr [esp + 4], eax",
        "call dword ptr [{original}]",
        "mov dword ptr [esp + 12], eax",
        "mov dword ptr [esp + 8], eax",
        "mov ecx, dword ptr [ebp + 8]",
        "mov dword ptr [esp], ecx",
        "mov ecx, dword ptr [ebp + 12]",
        "mov dword ptr [esp + 4], ecx",
        "call {record}",
        "mov eax, dword ptr [esp + 12]",
        "2:",
        "mov esp, ebp",
        "pop ebp",
        "ret",
        lookup = sym lookup,
        original = sym ORIGINAL,
        record = sym record,
    )
}

/// Replaces `mov eax,[edi+88h] ; test al,al ; jne skip` (edi = table, esp 16-byte aligned): rebuild when
/// [`should_rebuild`] says so, else skip; both with eax = count, as the original.
///
/// # Safety
/// Only jumped to from the rebuild check.
#[unsafe(naked)]
unsafe extern "C" fn rebuild_stub() {
    naked_asm!(
        "mov eax, dword ptr [edi + {count}]",
        "push eax",
        "push ecx",
        "push edx",
        "push eax",
        "call {should_rebuild}",
        "add esp, 4",
        "pop edx",
        "pop ecx",
        "test eax, eax",
        "pop eax",
        "jz 2f",
        "jmp dword ptr [{rebuild}]",
        "2:",
        "jmp dword ptr [{skip}]",
        count = const COUNT,
        should_rebuild = sym should_rebuild,
        rebuild = sym REBUILD_AT,
        skip = sym SKIP_AT,
    )
}

/// The intern's entry.
fn intern(core: &Image) -> Result<usize> {
    Ok(core.find_unique(BODY, "identifier intern")? - ENTRY_LEN)
}

pub(super) fn install_index(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = intern(core)?;
    core.require(f, ENTRY, "identifier intern entry")?;
    ORIGINAL.set(core.hook(f, ENTRY_LEN, intern_stub as unsafe extern "C" fn())?);
    init_index()?;
    Ok(core.report(&[f]))
}

pub(super) fn install_rebuild(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = intern(core)?;
    let site = core.find_unique_in(f, SPAN, CHECK, "intern rebuild check")?;
    let skip = core.jcc_at(site + JNE, Cond::NotEqual)?.target;
    core.require(skip, SKIP, "intern rebuild skip path")?;
    REBUILD_AT.set(site + CHECK_LEN);
    SKIP_AT.set(skip);
    let k = init_rebuild(cx);
    core.jump(site, CHECK_LEN, rebuild_stub as unsafe extern "C" fn())?;
    Ok(format!("{}, K={k}", core.report(&[site])))
}
