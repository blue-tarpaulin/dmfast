//! `resources`, Windows: byondcore's lookup walk jumps to [`walk_stub`], and the stores' remove function is hooked.

use std::arch::naked_asm;

use super::{NAME, REMOVE, init, lookup, remove_stub};
use crate::base::Result;
use crate::base::code::{Addr, Cond};
use crate::base::feature::Ctx;

/// The lookup: mov edx,[ebp+8] (store) ; or eax,-1 ; mov ecx,[ebp+10h] (name) ; mov [ebp-3Ch],ax ;
/// mov dword [ebp-34h],0 ; mov edi,[edx+84h] (first entry) ; (zero the locals) ; test edi,edi ; je notfound ; nop ;
/// walk: mov ebx,[edi+30h] (its name) ; mov eax,ecx ; mov ecx,ebx ; (inlined case-sensitive strcmp, as the index) ;
/// test eax,eax ; je match ; mov edi,[edi+0Ch] ; mov ecx,[ebp+10h] ; test edi,edi ; jne walk ; mov edx,[ebp+8] ;
/// notfound:
const LOOKUP: &str = "8B 55 08 83 C8 FF 8B 4D 10 66 89 45 C4 C7 45 CC 00 00 00 00 8B BA 84 00 00 00 \
                      C7 45 D0 00 00 00 00 C7 45 E0 00 00 00 00 C7 45 E4 00 00 00 00 C6 45 E8 00 \
                      85 FF 74 42 90 8B 5F 30 8B C1 8B CB 8A 11 3A 10 75 1A 84 D2 74 12 8A 51 01 3A 50 01 \
                      75 0E 83 C1 02 83 C0 02 84 D2 75 E4 33 C0 EB 05 1B C0 83 C8 01 85 C0 0F 84 ?? ?? ?? ?? \
                      8B 7F 0C 8B 4D 10 85 FF 75 C2 8B 55 08";
/// The replaced `test edi,edi ; je notfound ; nop`, and the walk after it.
const SITE: usize = 0x33;
const WALK: usize = 0x38;
const JE_NOT_FOUND: usize = 0x35;
const JE_MATCH: usize = 0x66;
/// The lookup's function starts this far before [`LOOKUP`].
const FUNC: usize = 0x28;
/// How far it reaches, for the remove call.
const SPAN: usize = 0x240;

/// The reload path's remove: push edi ; lea ecx,[ebx+80h] ; call find ; push eax ; lea ecx,[ebx+80h] ; call remove
const REMOVE_CALL: &str = "57 8D 8B 80 00 00 00 E8 ?? ?? ?? ?? 50 8D 8B 80 00 00 00 E8";
const CALL_REMOVE: usize = 19;
/// The remove function's moved entry: push ebp ; mov ebp,esp ; push -1.
const REMOVE_ENTRY: &str = "55 8B EC 6A FF";
const REMOVE_ENTRY_LEN: usize = 5;

/// [`walk_stub`]'s exits: the match code, not found, and the original walk.
static FOUND: Addr = Addr::new();
static NOT_FOUND: Addr = Addr::new();
static WALK_AT: Addr = Addr::new();

/// Replaces the walk's start: edx = store, ecx = name, edi = first entry. Found: edi = entry and ebx = its name, at
/// the match code. Not found: edi = 0, after the loop. Neither reads eax.
///
/// # Safety
/// Only jumped to from the lookup walk.
#[unsafe(naked)]
unsafe extern "C" fn walk_stub() {
    naked_asm!(
        "pushad",
        "push ecx",
        "push edx",
        "call {lookup}",
        "add esp, 8",
        "mov dword ptr [esp + 28], eax", // popad restores it into eax
        "popad",
        "cmp eax, 1",
        "je 3f",
        "test eax, eax",
        "jz 2f",
        "mov edi, eax",
        "mov ebx, dword ptr [edi + {name}]",
        "jmp dword ptr [{found}]",
        "2:",
        "xor edi, edi",
        "jmp dword ptr [{not_found}]",
        // Fallback: the moved `test edi,edi ; je notfound`, then the walk.
        "3:",
        "test edi, edi",
        "jz 2b",
        "jmp dword ptr [{walk}]",
        lookup = sym lookup,
        name = const NAME,
        found = sym FOUND,
        not_found = sym NOT_FOUND,
        walk = sym WALK_AT,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let m = core.find_unique(LOOKUP, "resource lookup")?;
    let call = core.find_unique_in(m - FUNC, SPAN, REMOVE_CALL, "resource remove call")?;
    let remove = core.call_target(call + CALL_REMOVE)?;
    core.require(remove, REMOVE_ENTRY, "resource remove entry")?;
    FOUND.set(core.jcc_at(m + JE_MATCH, Cond::Equal)?.target);
    NOT_FOUND.set(core.jcc_at(m + JE_NOT_FOUND, Cond::Equal)?.target);
    WALK_AT.set(m + WALK);
    REMOVE.set(core.hook(remove, REMOVE_ENTRY_LEN, remove_stub as unsafe extern "C" fn())?);
    init()?;
    core.jump(m + SITE, WALK - SITE, walk_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[m + SITE, remove]))
}
