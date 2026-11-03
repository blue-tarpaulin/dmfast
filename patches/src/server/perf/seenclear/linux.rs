//! `seenclear`, Linux: libbyond's clear (cdecl `tree`, eax = 0 after) jumps to [`clear_seen`], which matches it.

use super::{INSTALLED, clear_seen};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The clear: mov eax,[esp+4] ; mov eax,[eax+4] ; loop { and word [eax+4Ch],FFEFh ; mov eax,[eax+0Ch] } ; ret
const CLEAR: &str = "8B 44 24 04 8B 40 04 85 C0 74 11 8D 74 26 00 90 66 83 60 4C EF 8B 40 0C 85 C0 75 F4 C3";
/// The tree's node allocator (cdecl tree, record): free list +0Ch (its chunk from +14h node index >> +24h, chunks at
/// +18h, count +10h and the chunk's live +4 go up)...
const ALLOC: &str = "8B 7C 24 30 8B 74 24 34 8B 5F 0C 85 DB 0F 84 ?? ?? ?? ?? 8B 4F 24 8B 53 14 D3 EA C1 E2 03 03 57 18 \
                     83 47 10 01 83 42 04 01";
/// ...its new-chunk path: chunk count +14h (grown by 16), +20h slots of 5Ch bytes...
const CHUNK: &str =
    "8B 47 14 89 04 24 A8 0F 0F 84 ?? ?? ?? ?? 8B 6F 20 B8 FF FF FF FF 81 FD ?? ?? ?? ?? 77 ?? 6B C5 5C 83 C0 04";
/// ...and each new slot gets word +18h = FFFFh (free).
const INIT: &str = "BD FF FF FF FF C6 40 24 00 83 C0 5C 66 89 68 BC";
/// The pool's free: word +18h = FFFFh, onto the free list, count -1, its chunk's live -1 (then empty trailing
/// 5Ch-slot chunks are released).
const FREE: &str = "BD FF FF FF FF C7 43 10 00 00 00 00 66 89 6B 18 C7 43 0C 00 00 00 00 C7 43 08 00 00 00 00 C7 03 00 00 00 00 \
                    8B 47 0C 89 43 04 85 C0 74 02 89 18 83 6F 10 01 8B 4F 24 89 5F 0C 8B 43 14 D3 E8 89 C6 89 44 24 ?? \
                    8B 47 18 8D 14 F0 83 6A 04 01 8B 47 14 75 ?? 8D B6 00 00 00 00 83 F8 01 76 ?? 8B 4F 18 8D 14 C5 F8 FF FF FF \
                    8D 2C 11 8B 75 04 85 F6 75 ?? 8B 74 11 FC 85 F6 75 ?? 8B 47 20 8B 6D 00 85 C0 74 ?? 6B C0 5C";
/// How far from the allocator's start its new-chunk path, its slot loop, and the pool's free are.
const SPAN: usize = 0x400;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CLEAR, "seen-flag clear")?;
    let a = core.find_unique(ALLOC, "record pool allocator")?;
    let c = core.find_unique(CHUNK, "record pool chunk")?;
    let i = core.find_unique(INIT, "record pool slot init")?;
    let free = core.find_unique(FREE, "record pool free")?;
    ensure!(a < c && c < i && i - a < SPAN, "record pool allocator: pieces apart ({a:#x} {c:#x} {i:#x})");
    // The free belongs to the same pool code.
    ensure!(free.abs_diff(a) <= SPAN, "record pool free: not next to the allocator ({free:#x} {a:#x})");
    core.jump(f, 5, clear_seen as unsafe extern "C" fn(usize) -> u32)?;
    INSTALLED.commit(cx, ());
    Ok(core.report(&[f]))
}
