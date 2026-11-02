//! `indent`, Windows: the counting loop's tail jumps to [`count_loop`], which records each character and saves the
//! count at the loop's exit; where a line is accepted as code, [`check_line`] runs the check.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

use super::{BUF, CAP, check};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

/// The counting loop: call getc ; mov eax,[esi+18h] ; cmp eax,20h ; je ; cmp eax,9 ; jne exit ; inc edi ; jmp loop,
/// then cmp word [esi+36h],0.
const LOOP: &str = "8B CE E8 ?? ?? ?? ?? 8B 46 18 83 F8 20 74 05 83 F8 09 75 03 47 EB E9 66 83 7E 36 00";
/// Its tail from `cmp eax,20h` to the `jmp loop` (replaced), and its exit.
const TAIL: usize = 10;
const TAIL_LEN: usize = 13;
const EXIT: usize = 23;
/// Where a line is accepted as code: the unit size load, then the divisibility test whose failure reports
/// `inconsistent indentation` (message, error call; the caller pops both: movzx edi,[esi+38h] ; add esp,8).
const ACCEPT: &str = "0F B7 46 38 66 85 C0 74 09 66 83 7E 2C 00 8B D0 75 12 0F B7 C7 66 89 7E 38 8B D0 66 85 C0 0F 84 ?? \
                      ?? ?? ?? 0F B7 DF 0F B7 FA 8B C3 0F B7 CA 33 D2 F7 F1 85 D2 74 12 68 ?? ?? ?? ?? 56 E8 ?? ?? ?? ?? \
                      0F B7 7E 38 83 C4 08";
/// Its first two instructions (replaced: movzx eax,word [esi+38h] ; test ax,ax), the message push, and the error call.
const ACCEPT_LEN: usize = 7;
const MESSAGE_PUSH: usize = 0x38;
const REPORT_CALL: usize = 0x3E;

/// The leading whitespace count of the line, saved at the loop's exit.
static COUNT: AtomicU32 = AtomicU32::new(0);
static LOOP_HEAD: Foreign<unsafe extern "C" fn()> = Foreign::new();
static AFTER_LOOP: Foreign<unsafe extern "C" fn()> = Foreign::new();
static MESSAGE: AtomicUsize = AtomicUsize::new(0);
static REPORT: Foreign<unsafe extern "C" fn()> = Foreign::new();
static BACK: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// The counting loop's replaced tail (eax = the character, edi = the count), recording the character in [`BUF`] and
/// the count in [`COUNT`] at the exit.
///
/// # Safety
/// Only jumped to from the loop.
#[unsafe(naked)]
unsafe extern "C" fn count_loop() {
    naked_asm!(
        "cmp eax, 0x20",
        "je 2f",
        "cmp eax, 9",
        "jne 4f",
        "2:",
        "cmp edi, {cap}",
        "jae 3f",
        "mov byte ptr [edi + {buf}], al",
        "3:",
        "inc edi",
        "jmp dword ptr [{loop_head}]",
        "4:",
        "mov dword ptr [{count}], edi",
        "jmp dword ptr [{after_loop}]",
        cap = const CAP,
        buf = sym BUF,
        loop_head = sym LOOP_HEAD,
        count = sym COUNT,
        after_loop = sym AFTER_LOOP,
    )
}

/// Where a line is accepted as code (esi = the lexer): raise the error if [`check`] rejects the line, then the replaced
/// instructions.
///
/// # Safety
/// Only jumped to from the accepting code.
#[unsafe(naked)]
unsafe extern "C" fn check_line() {
    naked_asm!(
        "pushad",
        "push dword ptr [{count}]",
        "call {check}",
        "add esp, 4",
        "test eax, eax",
        "popad",
        "je 2f",
        "pushad",
        "push dword ptr [{message}]",
        "push esi",
        "call dword ptr [{report}]",
        "add esp, 8",
        "popad",
        "2:",
        "movzx eax, word ptr [esi + 0x38]",
        "test ax, ax",
        "jmp dword ptr [{back}]",
        count = sym COUNT,
        check = sym check,
        message = sym MESSAGE,
        report = sym REPORT,
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let l = core.find_unique(LOOP, "indentation count loop")?;
    let c = core.find_unique(ACCEPT, "indentation check")?;
    let message = core.ptr_at(c + MESSAGE_PUSH)?;
    core.expect(message, b"inconsistent indentation\0", "unexpected indentation message")?;
    core.untouched(&[(l + TAIL, TAIL_LEN), (c, ACCEPT_LEN)])?;
    LOOP_HEAD.set(l);
    AFTER_LOOP.set(l + EXIT);
    MESSAGE.store(message, Relaxed);
    REPORT.set(core.rel32(c + REPORT_CALL)?);
    BACK.set(c + ACCEPT_LEN);
    core.jump(l + TAIL, TAIL_LEN, count_loop as unsafe extern "C" fn())?;
    core.jump(c, ACCEPT_LEN, check_line as unsafe extern "C" fn())?;
    Ok(format!("at +{:#x}, +{:#x}", l - core.base, c - core.base))
}
