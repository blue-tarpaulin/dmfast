//! `num2sig`, Windows: the NUM2TEXT formatter is shared, so the two-argument path marks its calls and the
//! formatter's `sprintf` call goes through [`format`], which sends marked ones to [`sig_g`].
//!
//! The formatter takes `(value, digits, base)`, base `-1` meaning decimal, and only tests the base's sign. The
//! two-argument path now passes `-2`.

use std::arch::naked_asm;

use super::{SPRINTF, sig_g};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The two-argument path: `fstp [digits] ; movss xmm0,[digits] ; push -1 (the base) ; jmp` to the pushes of digits
/// and value.
const ARG: &str = "D9 9D CC FD FF FF F3 0F 10 85 CC FD FF FF 6A FF E9 ?? ?? ?? ?? BE FF FF 00 00 66 01 77 46";
/// Its `push -1`'s operand.
const BASE: usize = 15;
/// The base the two-argument path passes.
const MARK: i8 = -2;
/// The formatter's prologue: `push ebp ; mov ebp,esp ; sub esp,10h ; mov eax,[ebp+10h] (the base) ; mov ecx,eax ;
/// shr ecx,1Fh ; push esi ; xor cl,1 ; push edi ; mov [ebp-0Ch],ecx (the base isn't negative)`.
const PROLOGUE: &str = "55 8B EC 83 EC 10 8B 45 10 8B C8 C1 E9 1F 56 80 F1 01 57 89 4D F4";
/// Its decimal branch: `movss xmm0,[value] ; sub esp,8 ; cvtps2pd xmm0,xmm0 ; movsd [esp],xmm0 ; push esi (digits) ;
/// push "%.*g" ; push edi (buffer) ; call sprintf ; add esp,14h ; mov eax,edi ; ... ret`.
const DECIMAL: &str = "F3 0F 10 45 08 83 EC 08 0F 5A C0 F2 0F 11 04 24 56 68 ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? 83 C4 14 \
                       8B C7 5F 5E 8B E5 5D C3";
/// Its `call sprintf`.
const CALL: usize = 23;
/// The sign test (`cmp byte [ebp-0Ch],0 ; je`): a common one, so matches elsewhere are candidates, not relied on.
const SIGN: &str = "80 7D F4 00 0F 84 ?? ?? ?? ??";

/// The formatter's `sprintf` call: marked calls (base `-2` at `[ebp+10h]`) go to [`sig_g`], others to `sprintf`.
///
/// # Safety
/// Only called from the formatter's decimal branch.
#[unsafe(naked)]
unsafe extern "C" fn format() {
    naked_asm!(
        "cmp dword ptr [ebp + 0x10], {mark}",
        "jne 2f",
        "jmp {sig_g}",
        "2:",
        "jmp dword ptr [{sprintf}]",
        mark = const MARK,
        sig_g = sym sig_g,
        sprintf = sym SPRINTF,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let arg = core.find_unique(ARG, "num2text two-argument path")?;
    let decimal = core.find_unique(DECIMAL, "num2text %g formatter")?;
    let prologue = core.find_unique(PROLOGUE, "num2text formatter prologue")?;
    // The formatter's only sign test must lead to the decimal branch.
    let mut tests = Vec::new();
    for t in core.probe_all(SIGN)? {
        if (prologue..decimal).contains(&t) && core.rel32(t + 6)? == decimal {
            tests.push(t);
        }
    }
    let [test] = tests[..] else {
        bail!("num2text formatter: expected 1 sign test leading to %g, found {}", tests.len())
    };
    core.rely(test, SIGN.split_whitespace().count())?;
    let call = decimal + CALL;
    SPRINTF.set(core.rel32(call + 1)?);
    core.call(call, 5, format as unsafe extern "C" fn())?;
    core.write_code(arg + BASE, &[MARK as u8])?;
    Ok(format!("at +{:#x}, +{:#x}", arg - core.base, decimal - core.base))
}
