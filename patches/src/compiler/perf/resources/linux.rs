//! `resources`, Linux: libbyond's lookup walk jumps to [`walk_stub`], and the stores' remove function is hooked.

use std::arch::naked_asm;

use super::{NAME, REMOVE, init, lookup, remove_stub};
use crate::base::Result;
use crate::base::code::{Addr, Cond};
use crate::base::feature::Ctx;

/// The lookup: mov ebx,[edi+84h] (edi = store, ebx = first entry) ; mov [esp+0Ch],eax ; (zero the locals) ;
/// test ebx,ebx ; jne walk ; jmp empty ; (pad) ; next: mov ebx,[ebx+0Ch] ; test ebx,ebx ; je notfound ;
/// walk: mov ebp,[ebx+2Ch] (its name) ; sub esp,8 ; push esi (name) ; push ebp ; call strcmp ; add esp,10h ;
/// test eax,eax ; jne next ; found:
const LOOKUP: &str = "8B 9F 84 00 00 00 89 44 24 0C C7 44 24 30 00 00 00 00 C7 44 24 34 00 00 00 00 \
                      C7 44 24 38 00 00 00 00 C7 44 24 3C 00 00 00 00 C7 44 24 40 00 00 00 00 \
                      C7 44 24 44 00 00 00 00 C6 44 24 48 00 85 DB 75 15 E9 ?? ?? ?? ?? \
                      8D 74 26 00 90 8B 5B 0C 85 DB 0F 84 ?? ?? ?? ?? 8B 6B 2C 83 EC 08 56 55 E8 ?? ?? ?? ?? \
                      83 C4 10 85 C0 75 E1";
/// The replaced `test ebx,ebx ; jne walk ; jmp empty`.
const SITE: usize = 0x3F;
const JMP_EMPTY: usize = 0x43;
const JE_NOT_FOUND: usize = 0x52;
const WALK: usize = 0x58;
const FOUND_AT: usize = 0x6C;
/// The lookup's function starts this far before [`LOOKUP`].
const FUNC: usize = 0x23;
/// How far it reaches, for the remove call.
const SPAN: usize = 0x2A0;

/// The reload path's remove (cdecl `(store+80h, entry)`): sub esp,8 ; push eax ; push esi ; call remove
const REMOVE_CALL: &str = "83 EC 08 50 56 E8";
const CALL_REMOVE: usize = 5;
/// The remove function's moved entry: push esi ; xor edx,edx ; push ebx ; sub esp,4.
const REMOVE_ENTRY: &str = "56 31 D2 53 83 EC 04";
const REMOVE_ENTRY_LEN: usize = 7;

/// [`walk_stub`]'s exits: found, not found, the original walk, and the empty-list path.
static FOUND: Addr = Addr::new();
static NOT_FOUND: Addr = Addr::new();
static WALK_AT: Addr = Addr::new();
static EMPTY: Addr = Addr::new();

/// Replaces the walk's start: edi = store, esi = name, ebx = first entry. Found: ebx = entry and ebp = its name.
/// Not found: ebx = 0, after the loop. Neither reads eax.
///
/// # Safety
/// Only jumped to from the lookup walk.
#[unsafe(naked)]
unsafe extern "C" fn walk_stub() {
    naked_asm!(
        "pushad",
        "mov ebp, esp",
        "and esp, -16",
        "sub esp, 8",
        "push esi",
        "push edi",
        "call {lookup}",
        "mov esp, ebp",
        "mov dword ptr [esp + 28], eax", // popad restores it into eax
        "popad",
        "cmp eax, 1",
        "je 3f",
        "test eax, eax",
        "jz 2f",
        "mov ebx, eax",
        "mov ebp, dword ptr [ebx + {name}]",
        "jmp dword ptr [{found}]",
        "2:",
        "xor ebx, ebx",
        "jmp dword ptr [{not_found}]",
        // Fallback: the moved `test ebx,ebx ; jne walk ; jmp empty`.
        "3:",
        "test ebx, ebx",
        "jz 4f",
        "jmp dword ptr [{walk}]",
        "4:",
        "jmp dword ptr [{empty}]",
        lookup = sym lookup,
        name = const NAME,
        found = sym FOUND,
        not_found = sym NOT_FOUND,
        walk = sym WALK_AT,
        empty = sym EMPTY,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let m = core.find_unique(LOOKUP, "resource lookup")?;
    let call = core.find_unique_in(m - FUNC, SPAN, REMOVE_CALL, "resource remove call")?;
    let remove = core.call_target(call + CALL_REMOVE)?;
    core.require(remove, REMOVE_ENTRY, "resource remove entry")?;
    FOUND.set(m + FOUND_AT);
    NOT_FOUND.set(core.jcc_at(m + JE_NOT_FOUND, Cond::Equal)?.target);
    WALK_AT.set(m + WALK);
    EMPTY.set(core.branch_at(m + JMP_EMPTY)?.target);
    REMOVE.set(core.hook(remove, REMOVE_ENTRY_LEN, remove_stub as unsafe extern "C" fn())?);
    init()?;
    core.jump(m + SITE, JMP_EMPTY + 5 - SITE, walk_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[m + SITE, remove]))
}
