//! `indent`, Linux: the whitespace count (`ebp`, the character in `edx`) is incremented at two places, each sent through
//! a copy that also records the character; where a line is accepted as code, [`check_line`] runs the check.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use super::{BUF, CAP, check};
use crate::base::Result;
use crate::base::code::{Addr, Cond, Stub};
use crate::base::feature::Ctx;

/// The count's increments: mov eax,[ebx+24h] ; add ebp,1 ; test eax,eax (replaced), then the first character's test
/// or the loop's.
const FIRST: &str = "8B 43 24 83 C5 01 85 C0 74 ?? 0F B6 70 2A 89 F1 84 C9";
const NEXT: &str = "8B 43 24 83 C5 01 85 C0 75 ?? 83 FA 20 0F 84 ?? ?? ?? ?? 83 FA 09";
const COUNT_LEN: usize = 8;
/// Where a line is accepted as code: cmp edx,-1 ; je ; movzx esi,word [ebx+38h] ; movzx ecx,word [ebx+2Ch] (replaced) ;
/// test si,si ; jne ; mov [ebx+38h],bp ; test bp,bp ; jne
const ACCEPT: &str = "83 FA FF 0F 84 ?? ?? ?? ?? 0F B7 73 38 0F B7 4B 2C 66 85 F6 75 ?? 66 89 6B 38 66 85 ED 0F 85";
const ACCEPT_AT: usize = 9;
const ACCEPT_LEN: usize = 8;
/// The error report: sub esp,8 ; push message ; push ebx ; call report ; mov edx,[ebx+18h] ; add esp,10h ; cmp edx,-1
const REPORT: &str = "83 EC 08 68 ?? ?? ?? ?? 53 E8 ?? ?? ?? ?? 8B 53 18 83 C4 10 83 FA FF";
const MESSAGE_PUSH: usize = 4;
const REPORT_CALL: usize = 9;

static MESSAGE: AtomicUsize = AtomicUsize::new(0);
static REPORT_FN: Addr = Addr::new();
static BACK: Addr = Addr::new();

/// Where a line is accepted as code (ebx = the lexer, ebp = the count): raise the error if [`check`] rejects the line,
/// then the replaced instructions. The calls are 16-byte aligned whatever the entry alignment.
///
/// # Safety
/// Only jumped to from the accepting code.
#[unsafe(naked)]
unsafe extern "C" fn check_line() {
    naked_asm!(
        "pushad",
        "mov eax, ebp",
        "mov ebp, esp",
        "sub esp, 16",
        "and esp, -16",
        "mov dword ptr [esp], eax",
        "call {check}",
        "mov esp, ebp",
        "test eax, eax",
        "popad",
        "je 2f",
        "pushad",
        "mov ebp, esp",
        "sub esp, 16",
        "and esp, -16",
        "mov dword ptr [esp], ebx",
        "mov eax, dword ptr [{message}]",
        "mov dword ptr [esp + 4], eax",
        "call dword ptr [{report}]",
        "mov esp, ebp",
        "popad",
        "2:",
        "movzx esi, word ptr [ebx + 0x38]",
        "movzx ecx, word ptr [ebx + 0x2C]",
        "jmp dword ptr [{back}]",
        check = sym check,
        message = sym MESSAGE,
        report = sym REPORT_FN,
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let a = core.find_unique(FIRST, "indentation count (first)")?;
    let b = core.find_unique(NEXT, "indentation count (loop)")?;
    let c = core.find_unique(ACCEPT, "indentation check")? + ACCEPT_AT;
    let e = core.find_all(REPORT)?.first_or("indentation error report")?;
    let message = core.ptr_at(e + MESSAGE_PUSH)?;
    core.expect(message, b"inconsistent indentation\0", "unexpected indentation message")?;
    core.untouched(&[(a, COUNT_LEN), (b, COUNT_LEN), (c, ACCEPT_LEN)])?;
    // Each increment: cmp ebp,CAP ; jae tail ; mov [ebp+BUF],dl ; tail: the replaced instructions ; jmp back
    for site in [a, b] {
        let mut s = Stub::new();
        let tail = s.label();
        s.bytes(&[0x83, 0xFD, CAP as u8]) // cmp ebp,CAP
            .jcc8(Cond::AboveOrEqual, tail) // jae tail
            .bytes_u32(&[0x88, 0x95], BUF.as_ptr() as usize as u32, &[]) // mov [ebp+BUF],dl
            .bind(tail)
            .bytes(core.bytes(site, COUNT_LEN)?) // tail: mov eax,[ebx+24h] ; add ebp,1 ; test eax,eax
            .jmp(site + COUNT_LEN);
        let stub = s.exec()?;
        core.jump(site, COUNT_LEN, stub)?;
    }
    MESSAGE.store(message, Relaxed);
    REPORT_FN.set(core.call_target(e + REPORT_CALL)?);
    BACK.set(c + ACCEPT_LEN);
    core.jump(c, ACCEPT_LEN, check_line as unsafe extern "C" fn())?;
    Ok(core.report(&[a, b, c]))
}
