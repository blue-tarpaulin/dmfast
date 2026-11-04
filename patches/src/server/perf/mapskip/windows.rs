//! `mapskip`, Windows: the check (thiscall: ecx = record tree, [esp+4] = id, [esp+8] = object, [esp+0Ch] = next id;
//! callee pops 0Ch) is hooked by [`fast_stub`].

use std::arch::naked_asm;

use super::{INSTALLED, KEY, LEFT, LOOK, MASK, NEXT, RIGHT, verify};
use crate::base::code::{Cond, Foreign, read};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::server::layout::{RECORD_FLAGS, SEEN, THING_LOOK};

/// The check: SEH frame ; esi = tree ; [ebp-50h] = -1 ; init the temporary record at [ebp-68h] (call at +33h) ; its
/// key [ebp-4Ch] = id ; find or insert it (&inserted = [ebp+0Bh]; call at +4Fh) ; esi = record, none: return 0 ; not
/// inserted, record +30h == object +0A4h and record +2Ch == next (each `jne` to the update path): or word [esi+4Ch],10h
/// ; jmp tail (+7Fh), the update path right after it.
const CHECK: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 5C 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
                     64 A3 00 00 00 00 8B F1 83 C8 FF 8D 4D 98 66 89 45 B0 E8 ?? ?? ?? ?? 8B 45 08 8B CE 89 45 B4 8D 45 0B 50 \
                     8D 45 98 C7 45 FC 00 00 00 00 50 E8 ?? ?? ?? ?? 8B F0 85 F6 0F 84 ?? ?? ?? ?? 80 7D 0B 00 8B 55 10 8B 7D 0C \
                     75 ?? 8B 46 30 3B 87 A4 00 00 00 75 ?? 39 56 2C 75 ?? 66 83 4E 4C 10 E9";
/// Its first instructions, moved by the hook: push ebp ; mov ebp,esp ; push -1.
const ENTRY: usize = 5;
const INIT_CALL: usize = 0x33;
const FIND_CALL: usize = 0x4F;
const MISMATCH_JNES: [usize; 3] = [0x68, 0x73, 0x78];
const TAIL_JMP: usize = 0x7F;
/// The record initializer writes only the record (ecx): key +1Ch and appearance +20h = FFFFh, its last field the byte
/// at +5Ah.
const INIT: &str = "8B 15 ?? ?? ?? ?? B8 30 00 00 00 66 89 41 4C 33 C0 66 89 41 30 A1 ?? ?? ?? ?? 89 41 24 33 C0 C7 41 1C FF FF \
                    00 00 89 51 28 89 41 34 89 41 38 89 41 44 C7 41 2C FF FF 00 00 C6 41 5A 02 C7 41 20 FF FF 00 00 A1 ?? ?? ?? \
                    ?? 0F B7 80 ?? ?? 00 00 66 0F 6E C0 33 C0 0F 5B C0 89 41 3C F3 0F 11 41 48 A1 ?? ?? ?? ?? 0F B7 80 ?? ?? 00 \
                    00 66 89 41 40 A1 ?? ?? ?? ?? 0F B7 80 ?? ?? 00 00 66 89 41 42 33 C0 66 89 41 4E 89 41 50 C7 41 54 FF FF 00 \
                    00 66 89 41 58 66 89 41 32 C3";
/// Find or insert (thiscall: tree, &record, &inserted): descend from the root [ebx] by key +1Ch (unsigned), left +0 /
/// right +4; found: inserted = 0 and return the record, nothing written.
const FIND: &str = "55 8B EC 51 53 8B D9 56 33 F6 57 8B 13 85 D2 74 ?? 8B 4D 08 C7 45 FC 04 00 00 00 8B 49 1C 66 90 3B 4A 1C 73 \
                    05 83 CF FF EB 07 76 18 BF 01 00 00 00 33 C0 8B F2 85 FF 0F 4F 45 FC 8B 14 10 85 D2 75 DE EB ?? 8B 4D 0C 8B \
                    C2 5F 5E 5B C6 01 00 8B E5 5D C2 08 00";
/// The tail: return flags & 0DCFh after destroying the temporary record's appearance [ebp-48h] (its +20h; call at
/// +15h).
const TAIL: &str = "0F B7 76 4C 81 E6 CF 0D 00 00 8D 45 B8 C7 45 FC 02 00 00 00 50 E8 ?? ?? ?? ?? 83 C4 04 8B C6 8B 4D F4 64 89 \
                    0D 00 00 00 00 59 5F 5E 8B E5 5D C2 0C 00";
const DTOR_CALL: usize = 0x15;
/// That destructor: take the id, store FFFFh, and jump to the release.
const DTOR: &str = "55 8B EC 8B 45 08 8B 08 C7 00 FF FF 00 00 89 4D 08 5D E9";
const RELEASE_JMP: usize = 0x12;
/// The release, which returns at once (`je`) for FFFFh.
const RELEASE: &str = "55 8B EC 53 8B 5D 08 81 FB FF FF 00 00 0F 84";
const NONE_JE: usize = 0x0D;

type CheckFn = unsafe extern "thiscall" fn(usize, u32, usize, u32) -> u32;

/// The original check (its moved instructions, then the rest).
static ORIGINAL: Foreign<CheckFn> = Foreign::new();

/// The check's unchanged path without its temporary record; anything else goes to [`ORIGINAL`].
///
/// # Safety
/// Only jumped to from the check's start.
#[unsafe(naked)]
unsafe extern "C" fn fast_stub() {
    naked_asm!(
        "mov eax, dword ptr [ecx]",
        "test eax, eax",
        "jz 9f",
        "mov edx, dword ptr [esp + 4]",
        "2:",
        "cmp edx, dword ptr [eax + {key}]",
        "jb 3f",
        "ja 4f",
        "mov edx, dword ptr [esp + 8]",
        "mov edx, dword ptr [edx + {obj_look}]",
        "cmp edx, dword ptr [eax + {look}]",
        "jne 9f",
        "mov edx, dword ptr [esp + 12]",
        "cmp edx, dword ptr [eax + {next}]",
        "jne 9f",
        "or word ptr [eax + {flags}], {seen}",
        "movzx eax, word ptr [eax + {flags}]",
        "and eax, {mask}",
        "ret 12",
        "3:",
        "mov eax, dword ptr [eax + {left}]",
        "test eax, eax",
        "jnz 2b",
        "jmp 9f",
        "4:",
        "mov eax, dword ptr [eax + {right}]",
        "test eax, eax",
        "jnz 2b",
        "9:",
        "jmp dword ptr [{original}]",
        key = const KEY,
        obj_look = const THING_LOOK,
        look = const LOOK,
        next = const NEXT,
        flags = const RECORD_FLAGS,
        seen = const SEEN,
        mask = const MASK,
        left = const LEFT,
        right = const RIGHT,
        original = sym ORIGINAL,
    )
}

/// Verify mode's hook: the original, checked against the stub's answer.
///
/// # Safety
/// As the check's call.
unsafe extern "thiscall" fn verify_hook(tree: usize, id: u32, obj: usize, next: u32) -> u32 {
    // SAFETY: the check's arguments (caller): a live tree and object; ORIGINAL runs the check.
    unsafe { verify::check(tree, id, read(obj + THING_LOOK), next, || ORIGINAL.get()(tree, id, obj, next)) }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CHECK, "client record check")?;
    let tail = core.branch_at(f + TAIL_JMP)?;
    for at in MISMATCH_JNES {
        ensure!(core.jcc_at(f + at, Cond::NotEqual)?.target == tail.next, "client record check: unexpected branches");
    }
    core.follow_call(f + INIT_CALL, INIT, "record initializer")?;
    core.follow_call(f + FIND_CALL, FIND, "record find")?;
    core.require(tail.target, TAIL, "client record check tail")?;
    let dtor = core.follow_call(tail.target + DTOR_CALL, DTOR, "appearance destructor")?;
    let release = core.branch_at(dtor + RELEASE_JMP)?.target;
    core.require(release, RELEASE, "appearance release")?;
    core.require(core.jcc_at(release + NONE_JE, Cond::Equal)?.target, "5B 5D C3", "appearance release")?;

    let verifying = crate::base::driver::verify();
    let original = if verifying {
        core.hook(f, ENTRY, verify_hook as CheckFn)?
    } else {
        core.hook(f, ENTRY, fast_stub as unsafe extern "C" fn())?
    };
    ORIGINAL.set(original);
    INSTALLED.commit(cx, ());
    Ok(format!("{}{}", core.report(&[f]), if verifying { " (verify)" } else { "" }))
}
