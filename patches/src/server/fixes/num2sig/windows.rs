//! `num2sig`, Windows: the NUM2TEXT formatter is shared, so the two-argument path marks its calls and the
//! formatter's `sprintf` call goes through [`format`], which sends marked ones to [`sig_g`].
//!
//! The formatter takes `(value, digits, base)`, base `-1` meaning decimal, and only tests the base's sign. The
//! two-argument path now passes `-2`.

use std::arch::naked_asm;

use super::{SPRINTF, sig_g};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// The two-argument path, from the digits' store: `mov [digits],eax ; add esp,4 ; mov eax,[state] ; ... ; call` (the
/// value as a float) `; add esp,8 ; fstp [value] ; movss xmm0,[value] ; push -1` (the base).
const ARG: &str = "89 85 ?? ?? FF FF 83 C4 04 A1 ?? ?? ?? ?? 0F B7 48 46 8B 40 40 FF 74 C8 FC FF 74 C8 F8 E8 ?? ?? ?? \
                   ?? 83 C4 08 D9 9D CC FD FF FF F3 0F 10 85 CC FD FF FF 6A FF";
/// Its digits' slot (`[ebp+slot]`), its `fstp` and its `push -1`'s operand.
const DIGITS: usize = 2;
const FSTP: usize = 37;
const BASE: usize = 52;
/// The pushes of digits and value and the formatter's call, right after the `push -1` or where a `jmp` there goes:
/// `push [digits] ; push ecx ; movss [esp],xmm0 ; call formatter`.
const CALL_FORMAT: &str = "FF B5 ?? ?? FF FF 51 F3 0F 11 04 24 E8";
const CALL_DIGITS: usize = 2;
const CALL_AT: usize = 12;
/// The base the two-argument path passes.
const MARK: i8 = -2;
/// The formatter's sign of the base: `mov eax,[ebp+10h] (the base) ; mov ecx,eax ; shr ecx,1Fh`, then `xor cl,1 ;
/// mov [ebp+flag],ecx` (the base isn't negative), with `push esi` and `push edi` in between in some builds.
const SIGN_OF: &str = "8B 45 10 8B C8 C1 E9 1F";
/// Its decimal branch: `movss xmm0,[value] ; sub esp,8 ; cvtps2pd xmm0,xmm0 ; movsd [esp],xmm0 ; push esi (digits) ;
/// push "%.*g" ; push edi (buffer) ; call sprintf ; add esp,14h ; mov eax,edi`.
const DECIMAL: &str = "F3 0F 10 45 08 83 EC 08 0F 5A C0 F2 0F 11 04 24 56 68 ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? 83 C4 14 \
                       8B C7";
/// Its `call sprintf`.
const CALL: usize = 23;

/// The flag's `[ebp+flag]` after the base's sign at `at` ([`SIGN_OF`]).
fn flag_slot(core: &Image, at: usize) -> Result<u8> {
    let skip_pushes = |mut at: usize| -> Result<usize> {
        while matches!(core.u8_at(at)?, 0x56 | 0x57) {
            at += 1;
        }
        Ok(at)
    };
    let what = "num2text formatter: base sign not as expected";
    let xor = skip_pushes(at + SIGN_OF.split_whitespace().count())?;
    core.expect(xor, &[0x80, 0xF1, 0x01], what)?;
    let store = skip_pushes(xor + 3)?;
    core.expect(store, &[0x89, 0x4D], what)?;
    core.u8_at(store + 2)
}

/// The formatter's `sprintf` call: marked calls (base `-2` at `[ebp+10h]`) go to [`sig_g`], others to `sprintf`. Some
/// builds share this call with the radix branches, which reuse `[ebp+10h]` for the buffer: never `-2`.
///
/// # Safety
/// Only called from the formatter's `sprintf` call.
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
    // Then the pushes of digits (from the same slot) and value, and the call: right there or through a `jmp`.
    let mut pushes = arg + ARG.split_whitespace().count();
    if core.u8_at(pushes)? == 0xE9 {
        pushes = core.branch_at(pushes)?.target;
    }
    core.require(pushes, CALL_FORMAT, "num2text two-argument path's call")?;
    ensure!(
        core.bytes(pushes + CALL_DIGITS, 4)? == core.bytes(arg + DIGITS, 4)?,
        "num2text two-argument path: digits not as expected"
    );
    let formatter = core.call_target(pushes + CALL_AT)?;
    core.require(formatter, "55 8B EC", "num2text formatter's frame")?;
    let decimal = core.find_unique(DECIMAL, "num2text %g formatter")?;
    let sign = core.find_unique(SIGN_OF, "num2text formatter's base sign")?;
    let slot = flag_slot(core, sign)?;
    // The formatter's only test of that flag must lead to the decimal branch.
    let test_ida = format!("80 7D {slot:02X} 00 0F 84 ?? ?? ?? ??");
    let mut tests = Vec::new();
    for t in core.probe_all(&test_ida)? {
        if (sign..decimal).contains(&t) && core.rel32(t + 6)? == decimal {
            tests.push(t);
        }
    }
    let [test] = tests[..] else {
        bail!("num2text formatter: expected 1 sign test leading to %g, found {}", tests.len())
    };
    ensure!((formatter..test).contains(&sign), "num2text formatter: base sign not in the called formatter");
    core.rely(test, test_ida.split_whitespace().count())?;
    let call = decimal + CALL;
    SPRINTF.set(core.call_target(call)?);
    core.call(call, 5, format as unsafe extern "C" fn())?;
    core.write_code(arg + BASE, &[MARK as u8])?;
    Ok(core.report(&[arg + FSTP, decimal]))
}
