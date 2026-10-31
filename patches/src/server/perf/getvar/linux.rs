//! `getvar`, Linux: GetVariable's prologue is rewritten in place, the user var read's goes through [`var_read`].
//! Both install, or neither.

use std::arch::naked_asm;

use crate::base::code::{Foreign, branch};
use crate::base::feature::Ctx;
use crate::base::image::Hits;
use crate::base::{Result, bail};

const GETVAR: &str = "55 89 E5 57 56 53 83 EC 5C F3 0F 7E 4D 0C 8B 75 08 8B 5D 14 66 0F 6F C1 66 0F 7E CA 66 0F 73 D0 20 \
                      0F B6 C2 66 0F 7E 45 A8 3C ?? 77 ?? 66 0F 7E C7 FF 24 85";
const VAR_READ: &str = "55 57 56 53 83 EC 44 F3 0F 7E 44 24 5C 8B 5C 24 58 0F 29 44 24 08 FF 74 24 0C FF 74 24 0C E8";
/// The var read's error for a name the object doesn't have, pushed within [`ERROR_SPAN`] of its start (the other
/// prologue match is a cold twin without it).
const UNDEFINED: &[u8] = b"undefined variable %s/var/%s\0";
const ERROR_SPAN: usize = 0x400;

/// GetVariable at +9 (`ebp` frame; out, Value at +0Ch, ..): stock's `movq xmm1,[ebp+0Ch] ; mov esi,[ebp+8] ;
/// mov ebx,[ebp+14h] ; movdqa xmm0,xmm1 ; movd edx,xmm1 ; psrlq xmm0,32` as dword loads: edx = lo,
/// xmm1 = lo|hi<<32, xmm0 = hi, esi and ebx as before, no flags touched.
#[rustfmt::skip]
const GETVAR_NEW: [u8; 24] = [
    0x8B, 0x55, 0x0C,             // mov edx,[ebp+0Ch]
    0x66, 0x0F, 0x6E, 0x4D, 0x0C, // movd xmm1,[ebp+0Ch]
    0x66, 0x0F, 0x6E, 0x45, 0x10, // movd xmm0,[ebp+10h]
    0x66, 0x0F, 0x62, 0xC8,       // punpckldq xmm1,xmm0
    0x8B, 0x75, 0x08,             // mov esi,[ebp+8]
    0x8B, 0x5D, 0x14,             // mov ebx,[ebp+14h]
    0x90,                         // nop
];

/// The var read's replaced instructions run from +7 to its first call at +30.
const READ_AT: usize = 7;
const READ_BACK: usize = 30;
/// Its stack offsets after `sub esp,44h` (literals in [`VAR_READ`]): out, the Value, its copy.
const OUT: usize = 0x58;
const VALUE: usize = 0x5C;
const COPY: usize = 0x08;

/// The var read's call after the replaced instructions.
static BACK: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// The var read's `movq xmm0,[esp+5Ch] ; mov ebx,[esp+58h] ; movaps [esp+8],xmm0 ; push [esp+0Ch] ; push [esp+0Ch]`
/// with dword and word loads (SSE2 has no dword insert): xmm0 = Value (lo, hi, 0, 0), ebx = out, the 16-byte copy at
/// [esp+8], then hi and lo pushed from the argument.
///
/// # Safety
/// Only jumped to from the var read.
#[unsafe(naked)]
unsafe extern "C" fn var_read() {
    naked_asm!(
        "movd xmm0, dword ptr [esp + {lo}]",
        "pinsrw xmm0, word ptr [esp + {hi}], 2",
        "pinsrw xmm0, word ptr [esp + {hi} + 2], 3",
        "mov ebx, dword ptr [esp + {out}]",
        "movaps xmmword ptr [esp + {copy}], xmm0",
        "push dword ptr [esp + {hi}]",
        "push dword ptr [esp + {hi}]",
        "jmp dword ptr [{back}]",
        lo = const VALUE,
        hi = const VALUE + 4,
        out = const OUT,
        copy = const COPY,
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let getvar = core.find_unique(GETVAR, "GetVariable")?;
    // The var read: the prologue match that pushes the undefined-variable format. The other match and the pushes
    // elsewhere are candidates, not relied on.
    let text = core.find_bytes(UNDEFINED)?.unique("undefined variable format")?;
    let push = [&[0x68][..], &(text as u32).to_le_bytes()].concat();
    let pushes = core.probe_bytes(&push)?;
    let near = |f: usize| pushes.iter().copied().filter(move |&p| p > f && p - f < ERROR_SPAN);
    let reads: Hits = core.probe_all(VAR_READ)?.into_iter().filter(|&f| near(f).next().is_some()).collect();
    let read = reads.unique("user var read")?;
    core.rely(read, VAR_READ.split_whitespace().count())?;
    for p in near(read) {
        core.rely(p, push.len())?;
    }

    // jmp var_read, then int3 up to the call.
    let mut r = branch(0xE9, read + READ_AT, var_read as unsafe extern "C" fn() as usize, 5);
    r.resize(READ_BACK - READ_AT, 0xCC);
    let sites = [(getvar + 9, &GETVAR_NEW[..]), (read + READ_AT, &r[..])];
    let ranges = sites.map(|(at, code)| (at, code.len()));
    core.untouched(&ranges)?;
    // Nor may a relocated word (a jump table entry, a function pointer) point inside one (all read, none relied on).
    for s in core.reloc_sites() {
        let v = u32::from_le_bytes(*core.haystack(s, 4)?.first_chunk().ok_or("short read")?) as usize;
        if ranges.iter().any(|&(at, len)| (at + 1..at + len).contains(&v)) {
            bail!("an address inside a rewritten range, at {s:#x}");
        }
    }
    BACK.set(read + READ_BACK);
    for (at, code) in sites {
        core.write_code(at, code)?;
    }
    Ok(format!("GetVariable +{:#x}, user var read +{:#x}", getvar - core.base, read - core.base))
}
