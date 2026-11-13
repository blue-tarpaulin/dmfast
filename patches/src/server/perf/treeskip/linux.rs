//! `treeskip`, Linux: libbyond's send loads each walk's list through a call of [`unseen_stub`], [`placed_stub`], or
//! [`stub`].

use std::arch::naked_asm;

use super::{PLACED, scan, walk};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The send's start (regparm: ax = client, edx = record tree esi, ecx = message ebx), through the unseen-record walk:
/// eax = the list [esi+4], `lea esi,[ebx+4] ; test eax,eax` (replaced), `je`, then its loop testing the seen flag.
const SEND: &str = "55 0F B7 C0 57 56 89 D6 53 89 CB 81 EC 48 04 00 00 89 54 24 1C 89 44 24 34 50 E8 ?? ?? ?? ?? 89 44 24 \
                    24 83 C4 10 85 C0 0F 84 ?? ?? ?? ?? 8B 84 24 50 04 00 00 83 EC 0C C7 00 00 00 00 00 56 E8 ?? ?? ?? ?? \
                    8B 46 04 83 C4 10 8D 73 04 85 C0 0F 84 ?? ?? ?? ?? BA FA 00 00 00 EB ?? 8D 74 26 00 83 EA 01 0F B7 CA \
                    89 44 8C 48 8B 40 0C 85 C0 74 ?? F6 40 4C 10 75 F3";
const UNSEEN_LOAD: &str = "8D 73 04 85 C0";
/// The send's length (to its last `jmp`).
const LEN: usize = 0x11E9;
/// The flag-2 walk: eax = tree [esp+10h], eax = the list, `mov word [esp+20h],dx` (replaced), `jne` (+1Bh) to its
/// loop testing flag 2.
const PLACED_WALK: &str = "8B 44 24 20 BA FA 00 00 00 83 C4 10 8B 40 04 66 89 54 24 20 BA FA 00 00 00 85 C0 75 ?? E9";
const PLACED_STORE: usize = 0x0F;
const PLACED_JNE: usize = 0x1B;
/// What the flag-2 walk does to its records: flags & ~22h | 4.
const PLACED_SETS: &str = "83 E0 DD 83 C8 04 66 89 47 4C";
/// The flag-4 walk: eax = tree, then the list load `mov eax,[eax+4] ; test eax,eax` (+7, replaced) and `jne` (+0Ch)
/// to its loop.
const FIRST_WALK: &str = "8B 44 24 20 83 C4 10 8B 40 04 85 C0 75 ?? E9";
const FIRST_LOAD: usize = 0x07;
const FIRST_JNE: usize = 0x0C;
/// The other flag walks: eax = tree [esp+10h], the list load (+4, replaced), `je` (near or short) to the walk's
/// end, and `jmp` to its loop.
const WALKS_AT: [(&str, usize); 2] = [
    ("8B 44 24 10 8B 40 04 85 C0 0F 84 ?? ?? ?? ?? BD FA 00 00 00 EB ??", 0x14),
    ("8B 44 24 10 8B 40 04 85 C0 74 ?? BD FA 00 00 00 EB ??", 0x10),
];
const WALK_LOAD: usize = 0x04;

type Stub = unsafe extern "C" fn();

/// The flag walks, each with its stub.
const WALKS: [(u16, Stub); 8] = [
    (0x001, stub::<0x001>),
    (0x004, stub::<0x004>),
    (0x008, stub::<0x008>),
    (0x040, stub::<0x040>),
    (0x080, stub::<0x080>),
    (0x100, stub::<0x100>),
    (0x400, stub::<0x400>),
    (0x800, stub::<0x800>),
];

/// The unseen-record walk's `lea esi,[ebx+4] ; test eax,eax` with eax = [`scan`] of the list eax; ebx, ecx, and edx
/// kept, [`scan`] called on a 16-byte aligned stack.
///
/// # Safety
/// Only called from the unseen-record walk's start.
#[unsafe(naked)]
unsafe extern "C" fn unseen_stub() {
    naked_asm!(
        "lea esi, [ebx + 4]",
        "push ebx",
        "push ecx",
        "push edx",
        "mov ebx, esp",
        "and esp, -16",
        "sub esp, 12",
        "push eax",
        "call {scan}",
        "mov esp, ebx",
        "pop edx",
        "pop ecx",
        "pop ebx",
        "test eax, eax",
        "ret",
        scan = sym scan,
    )
}

/// The flag-2 walk's `mov word [esp+20h],dx` with eax = [`walk`] of the list eax for flag 2; as [`unseen_stub`].
///
/// # Safety
/// Only called from the flag-2 walk's start.
#[unsafe(naked)]
unsafe extern "C" fn placed_stub() {
    naked_asm!(
        "mov word ptr [esp + 0x24], dx",
        "push ebx",
        "push ecx",
        "push edx",
        "mov ebx, esp",
        "and esp, -16",
        "sub esp, 8",
        "push {flag}",
        "push eax",
        "call {walk}",
        "mov esp, ebx",
        "pop edx",
        "pop ecx",
        "pop ebx",
        "ret",
        flag = const PLACED,
        walk = sym walk,
    )
}

/// A flag walk's list load (eax = tree): eax = [`walk`] of the list for `FLAG`, and `test eax,eax`; as
/// [`unseen_stub`].
///
/// # Safety
/// Only called from a flag walk's list load.
#[unsafe(naked)]
unsafe extern "C" fn stub<const FLAG: u32>() {
    naked_asm!(
        "push ebx",
        "push ecx",
        "push edx",
        "mov ebx, esp",
        "and esp, -16",
        "sub esp, 8",
        "push {flag}",
        "push dword ptr [eax + {first}]",
        "call {walk}",
        "mov esp, ebx",
        "pop edx",
        "pop ecx",
        "pop ebx",
        "test eax, eax",
        "ret",
        flag = const FLAG,
        first = const super::FIRST,
        walk = sym walk,
    )
}

/// The flag a walk's loop at `at` tests: `test byte [eax+4Ch or 4Dh],imm8 ; je`.
fn flag_at(core: &Image, at: usize) -> Result<u16> {
    let [op, modrm, disp, imm, je] = core.array::<5>(at)?;
    ensure!(op == 0xF6 && modrm == 0x40 && (disp == 0x4C || disp == 0x4D) && je == 0x74, "flag walk: unexpected loop");
    core.rely(at, 5)?;
    Ok(u16::from(imm) << (8 * (disp - 0x4C)))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(SEND, "record change send")?;
    let p = core.find_unique_in(f, LEN, PLACED_WALK, "flag-2 walk")?;
    ensure!(flag_at(core, core.branch_at(p + PLACED_JNE)?.target)? == PLACED, "flag-2 walk: not flag 2");
    ensure!(!core.find_in(f, LEN, PLACED_SETS)?.is_empty(), "flag-2 walk's flags: not found");
    let first = core.find_unique_in(f, LEN, FIRST_WALK, "flag-4 walk")?;
    let mut loads = vec![(first + FIRST_LOAD, flag_at(core, core.branch_at(first + FIRST_JNE)?.target)?)];
    for (sig, jmp) in WALKS_AT {
        for &at in core.find_in(f, LEN, sig)?.iter() {
            loads.push((at + WALK_LOAD, flag_at(core, core.branch_at(at + jmp)?.target)?));
        }
    }
    let mut flags: Vec<u16> = loads.iter().map(|l| l.1).collect();
    flags.sort_unstable();
    ensure!(flags.iter().eq(WALKS.iter().map(|w| &w.0)), "flag walks: unexpected flags {flags:x?}");

    let unseen = f + SEND.find(UNSEEN_LOAD).unwrap() / 3;
    let mut sites = vec![unseen, p + PLACED_STORE];
    core.untouched(&[(unseen, 5)])?;
    core.call(unseen, 5, unseen_stub as Stub)?;
    core.call(p + PLACED_STORE, 5, placed_stub as Stub)?;
    for (at, flag) in loads {
        core.untouched(&[(at, 5)])?;
        core.call(at, 5, WALKS.iter().find(|w| w.0 == flag).unwrap().1)?;
        sites.push(at);
    }
    Ok(core.report(&sites))
}
