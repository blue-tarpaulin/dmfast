//! `seenclear`, Windows: byondcore's clear (`__fastcall`, ecx = tree) jumps to [`stub`].

use std::arch::naked_asm;

use super::{INSTALLED, clear_seen};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The clear: mov eax,[ecx+4] ; test eax,eax ; jz ret ; mov ecx,FFEFh ; loop { and [eax+4Ch],cx ;
/// mov eax,[eax+0Ch] } ; ret
const CLEAR: &str = "8B 41 04 85 C0 74 14 B9 EF FF 00 00 0F 1F 40 00 66 21 48 4C 8B 40 0C 85 C0 75 F5 C3";
/// The tree's node allocator: free list +0Ch, chunk count +14h (grown by 16, realloc n*8+80h of +18h), slots per
/// chunk +20h, node size 5Ch.
const ALLOC: &str = "8B F9 8B 77 0C 85 F6 74 ?? 8B 5E 14 8B 4F 24 D3 EB E9 ?? ?? ?? ?? 8B 5F 14 89 5D ?? F6 C3 0F 75 ?? \
                     8D 04 DD 80 00 00 00 50 FF 77 18 FF 15 ?? ?? ?? ?? 83 C4 08 85 C0 0F 84 ?? ?? ?? ?? 8B 5F 14 89 47 18 \
                     89 5D ?? 8B 47 20 33 C9 89 45 ?? BA 5C 00 00 00";
/// The pool's free: word +18h = FFFFh, onto the free list, count -1, its chunk's live -1.
const FREE: &str = "83 C8 FF C7 43 10 00 00 00 00 66 89 43 18 C7 43 0C 00 00 00 00 C7 43 08 00 00 00 00 C7 03 00 00 00 00 \
                    8B 47 0C 89 43 04 8B 47 0C 85 C0 74 02 89 18 FF 4F 10 8B 4F 24 8B 47 18 89 5F 0C 8B 53 14 D3 EA 89 55 ?? \
                    83 44 D0 04 FF";
/// How far after its allocator the pool's frees are.
const SPAN: usize = 0x2000;

/// Runs [`clear_seen`] and leaves the registers as the clear does: eax = 0, ecx = FFEFh unless the list is empty, edx
/// kept.
///
/// # Safety
/// Only jumped to from the clear.
#[unsafe(naked)]
unsafe extern "C" fn stub() {
    naked_asm!(
        "push edx",
        "push ecx", // the tree
        "call {clear}",
        "pop ecx",
        "pop edx",
        "cmp dword ptr [ecx + 4], 0",
        "je 2f",
        "mov ecx, 0xFFEF",
        "2:",
        "ret",
        clear = sym clear_seen,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CLEAR, "seen-flag clear")?;
    let a = core.find_unique(ALLOC, "record pool allocator")?;
    // Other pools inline the same free (not relied on); this pool's own frees are next to its allocator.
    let own: Vec<usize> = core.probe_all(FREE)?.into_iter().filter(|x| (a + 1..a + SPAN).contains(x)).collect();
    ensure!(!own.is_empty(), "record pool free: not found next to the allocator");
    for x in own {
        core.rely(x, FREE.split_whitespace().count())?;
    }
    core.jump(f, 5, stub as unsafe extern "C" fn())?;
    INSTALLED.commit(cx, ());
    Ok(core.report(&[f]))
}
