//! `mapskip`, Linux: the check (cdecl: record tree, id, object, next id) is hooked by [`fast_stub`].

use std::arch::naked_asm;

use super::{KEY, LEFT, LOOK, MASK, NEXT, RIGHT, verify};
use crate::base::code::{Cond, Foreign, read};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::server::layout::{RECORD_FLAGS, SEEN, THING_LOOK};

/// The check: esi = object ; the temporary record at [ebp-78h] filled in place ([ebp-58h], its appearance +20h, =
/// FFFFh ; key [ebp-5Ch] = id) ; find or insert it (tree, &record, &inserted [ebp-79h]; call at +0BAh) ; ebx =
/// record, none: return 0 ; eax = object +0A0h ; not inserted: `je` (+0D6h) the unchanged check, else the update path
/// right after it.
const CHECK: &str = "55 B8 FF FF FF FF BA FF FF 00 00 89 E5 57 56 53 81 EC 90 00 00 00 66 89 45 A0 31 C0 8B 75 10 89 45 B0 \
                     A1 ?? ?? ?? ?? 89 55 B4 0F B7 90 ?? ?? 00 00 C6 45 AC 00 C6 45 E2 02 66 0F 6E C2 C7 45 A8 FF FF 00 00 \
                     66 0F D6 85 70 FF FF FF 0F B7 90 ?? ?? 00 00 0F B7 80 ?? ?? 00 00 DF AD 70 FF FF FF C7 45 BC 00 00 00 00 \
                     C1 E2 10 C7 45 C0 00 00 00 00 09 D0 31 D2 C7 45 CC 00 00 00 00 89 45 C8 8B 45 0C C7 45 C4 00 00 00 00 \
                     89 45 A4 8D 45 87 50 8D 45 88 C7 45 D4 30 00 00 00 C7 45 D8 00 00 00 00 C7 45 DC FF FF 00 00 66 89 55 E0 \
                     C7 45 B8 00 00 00 00 D9 5D D0 50 FF 75 08 E8 ?? ?? ?? ?? 89 C3 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? \
                     80 7D 87 00 8B 86 A0 00 00 00 0F 84";
/// Its first instructions, moved by the hook: push ebp ; mov eax,-1.
const ENTRY: usize = 6;
const FIND_CALL: usize = 0xBA;
const SAME_JE: usize = 0xD6;
/// Find or insert (cdecl: tree, &record, &inserted): descend from the root [esi] by key +1Ch (unsigned), left +0 /
/// right +4; found: inserted = 0 and return the record, nothing written.
const FIND: &str = "55 57 56 53 83 EC 0C 8B 74 24 20 8B 4C 24 24 8B 1E 85 DB 0F 84 ?? ?? ?? ?? 8B 51 1C 3B 53 1C 72 17 77 2D \
                    8B 44 24 28 C6 00 00 83 C4 0C 89 D8 5B 5E 5F 5D C3 8D 74 26 00 89 D8 BF FF FF FF FF 8B 00 85 C0 74 1B 89 C3 \
                    EB D3 8D B4 26 00 00 00 00 8D 43 04 BF 01 00 00 00 EB E5";
/// The unchanged check: record +30h == eax and +2Ch == next (each `jne` to the update path); or word [ebx+4Ch],10h ;
/// jmp tail.
const SAME: &str = "39 43 30 0F 85 ?? ?? ?? ?? 8B 7D 14 39 7B 2C 0F 85 ?? ?? ?? ?? 0F B7 43 4C 83 C8 10 66 89 43 4C EB";
const MISMATCH_JNES: [usize; 2] = [0x03, 0x0F];
const TAIL_JMP: usize = 0x20;
/// The tail: return flags & 0DCFh after releasing the temporary record's appearance [ebp-58h] (call at +0Fh).
const TAIL: &str = "89 C3 81 E3 CF 0D 00 00 8D 45 A8 83 EC 0C 50 E8 ?? ?? ?? ?? 83 C4 10 8D 65 F4 89 D8 5B 5E 5F 5D C3";
const RELEASE_CALL: usize = 0x0F;
/// That release: take the id, store FFFFh; FFFFh returns at once (`je`).
const RELEASE: &str = "53 83 EC 08 8B 44 24 10 8B 18 C7 00 FF FF 00 00 81 FB FF FF 00 00 74";
const NONE_JE: usize = 0x16;

type CheckFn = unsafe extern "C" fn(usize, u32, usize, u32) -> u32;

/// The original check (its moved instructions, then the rest).
static ORIGINAL: Foreign<CheckFn> = Foreign::new();

/// The check's unchanged path without its temporary record; anything else goes to [`ORIGINAL`].
///
/// # Safety
/// Only jumped to from the check's start.
#[unsafe(naked)]
unsafe extern "C" fn fast_stub() {
    naked_asm!(
        "mov eax, dword ptr [esp + 4]",
        "mov eax, dword ptr [eax]",
        "test eax, eax",
        "jz 9f",
        "mov edx, dword ptr [esp + 8]",
        "2:",
        "cmp edx, dword ptr [eax + {key}]",
        "jb 3f",
        "ja 4f",
        "mov ecx, dword ptr [esp + 12]",
        "mov ecx, dword ptr [ecx + {obj_look}]",
        "cmp ecx, dword ptr [eax + {look}]",
        "jne 9f",
        "mov ecx, dword ptr [esp + 16]",
        "cmp ecx, dword ptr [eax + {next}]",
        "jne 9f",
        "or word ptr [eax + {flags}], {seen}",
        "movzx eax, word ptr [eax + {flags}]",
        "and eax, {mask}",
        "ret",
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
unsafe extern "C" fn verify_hook(tree: usize, id: u32, obj: usize, next: u32) -> u32 {
    // SAFETY: the check's arguments (caller): a live tree and object; ORIGINAL runs the check.
    unsafe { verify::check(tree, id, read(obj + THING_LOOK), next, || ORIGINAL.get()(tree, id, obj, next)) }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CHECK, "client record check")?;
    core.follow_call(f + FIND_CALL, FIND, "record find")?;
    let same = core.jcc_at(f + SAME_JE, Cond::Equal)?;
    core.require(same.target, SAME, "client record check's unchanged path")?;
    for at in MISMATCH_JNES {
        ensure!(
            core.jcc_at(same.target + at, Cond::NotEqual)?.target == same.next,
            "client record check: unexpected unchanged path"
        );
    }
    let tail = core.branch_at(same.target + TAIL_JMP)?.target;
    core.require(tail, TAIL, "client record check tail")?;
    let release = core.follow_call(tail + RELEASE_CALL, RELEASE, "appearance release")?;
    core.require(core.jcc_at(release + NONE_JE, Cond::Equal)?.target, "83 C4 08 5B C3", "appearance release")?;

    let verifying = crate::base::driver::verify();
    let original = if verifying {
        core.hook(f, ENTRY, verify_hook as CheckFn)?
    } else {
        core.hook(f, ENTRY, fast_stub as unsafe extern "C" fn())?
    };
    ORIGINAL.set(original);
    Ok(format!("{}{}", core.report(&[f]), if verifying { " (verify)" } else { "" }))
}
