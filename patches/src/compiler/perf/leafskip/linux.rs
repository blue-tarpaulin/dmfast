//! `leafskip`, Linux: the pass function (`node` in eax, `mode` in edx) is hooked by [`visit_stub`]. Its handlers test
//! the finished flag and child count ([`crate::compiler::layout`]) with `test byte [ebx+11h],2 ; jne return ;
//! movzx eax,word [ebx+1Eh]`, which install checks.

use std::arch::naked_asm;

use crate::base::code::{Addr, Cond};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::layout::{COUNT, FLAGS};

/// push ebp ; mov ebp,esp ; push edi ; mov edi,edx ; push esi ; push ebx ; mov ebx,eax ; sub esp,5Ch ;
/// cmp byte [eax+4],6 ; ja ... ; movzx eax,byte [eax+4] ; jmp [table+eax*4]
const PASS: &str = "55 89 E5 57 89 D7 56 53 89 C3 83 EC 5C 80 78 04 06 77 ?? 0F B6 40 04 FF 24 85";
/// The hooked entry: push ebp ; mov ebp,esp ; push edi ; mov edi,edx
const ENTRY: usize = 6;
/// A handler's finished test: test byte [ebx+11h],2 ; jne return ; movzx eax,word [ebx+1Eh]
const TEST: &str = "F6 43 11 02 0F 85 ?? ?? ?? ?? 0F B7 43 1E";
/// The return it jumps to: lea esp,[ebp-0Ch] ; pop ebx ; pop esi ; pop edi ; pop ebp ; ret
const RETURN: &[u8] = &[0x8D, 0x65, 0xF4, 0x5B, 0x5E, 0x5F, 0x5D, 0xC3];
const FINISHED: u8 = 2;

/// The original pass function.
static ORIGINAL: Addr = Addr::new();

/// # Safety
/// Only jumped to from the pass function's entry.
#[unsafe(naked)]
unsafe extern "C" fn visit_stub() {
    naked_asm!(
        "test byte ptr [eax + {flags}], {finished}",
        "jz 2f",
        "cmp word ptr [eax + {count}], 0",
        "jne 2f",
        "ret",
        "2:",
        "jmp dword ptr [{original}]",
        original = sym ORIGINAL,
        flags = const FLAGS,
        finished = const FINISHED,
        count = const COUNT,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(PASS, "compile pass function")?;
    let tests = core.find_in(f, 0x1000, TEST)?;
    let mut ok = !tests.is_empty();
    for &t in &tests {
        let ret = core.jcc_at(t + 4, Cond::NotEqual)?.target;
        ok &= core.bytes(ret, RETURN.len())? == RETURN;
    }
    ensure!(ok, "compile pass function: finished flag or child count not where expected");
    ORIGINAL.set(core.hook(f, ENTRY, visit_stub as unsafe extern "C" fn())?);
    Ok(core.report(&[f]))
}
