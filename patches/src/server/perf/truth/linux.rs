//! `truth`, Linux: each test's number branch (at +40h) is rewritten in place.

use crate::base::Result;
use crate::base::code::pad;
use crate::base::feature::Ctx;

/// The true test (cdecl Value; `al` = result). Number branch: mov [esp],eax ; fld dword [esp] ; mov edx,1 ; fldz ;
/// fucomip st,st1 ; fstp st0 ; setp al ; cmovne eax,edx ; add esp,4 ; ret
const IS_TRUE: &str = "83 EC 04 8B 54 24 08 8B 44 24 0C 84 D2 74 21 80 FA 22 74 1C 80 FA 2A 74 27 85 C0 0F 95 C0 80 FA 06 \
                       0F 95 C2 83 C4 04 09 D0 C3 8D B6 00 00 00 00 31 C0 83 C4 04 C3 8D B4 26 00 00 00 00 8D 76 00 \
                       89 04 24 D9 04 24 BA 01 00 00 00 D9 EE DF E9 DD D8 0F 9A C0 0F 45 C2 83 C4 04 C3";
/// The false test (same shape, the number in `ecx`). Number branch: fldz ; mov [esp],ecx ; fld dword [esp] ;
/// fucomip st,st1 ; fstp st0 ; jp .. ; mov eax,1 ; je .. ; jmp .. (to its `al = !ecx` tail and its return)
const IS_FALSE: &str = "83 EC 04 8B 54 24 08 8B 4C 24 0C 84 D2 74 21 80 FA 22 74 1C 80 FA 2A 74 27 85 C9 0F 94 C0 80 FA 06 \
                        0F 94 C2 83 C4 04 21 D0 C3 8D B6 00 00 00 00 B8 01 00 00 00 83 C4 04 C3 8D B4 26 00 00 00 00 \
                        D9 EE 89 0C 24 D9 04 24 DF E9 DD D8 7A CB B8 01 00 00 00 74 E0 EB C2";
/// Where each test's number branch starts.
const NUMBER: usize = 0x40;

/// The true test's new number branch, padded to the stock branch's length.
#[rustfmt::skip]
const TRUE_NEW: [u8; 27] = pad(&[
    0x25, 0xFF, 0xFF, 0xFF, 0x7F, // and eax,7FFFFFFFh
    0x0F, 0x95, 0xC0,             // setne al
    0x0F, 0xB6, 0xC0,             // movzx eax,al
    0x83, 0xC4, 0x04,             // add esp,4
    0xC3,                         // ret
]);

/// The false test's new number branch, padded to the stock branch's length.
#[rustfmt::skip]
const FALSE_NEW: [u8; 23] = pad(&[
    0x31, 0xC0,                         // xor eax,eax
    0xF7, 0xC1, 0xFF, 0xFF, 0xFF, 0x7F, // test ecx,7FFFFFFFh
    0x0F, 0x94, 0xC0,                   // sete al
    0x83, 0xC4, 0x04,                   // add esp,4
    0xC3,                               // ret
]);

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let t = core.find_unique(IS_TRUE, "true test")? + NUMBER;
    let f = core.find_unique(IS_FALSE, "false test")? + NUMBER;
    core.untouched(&[(t, TRUE_NEW.len()), (f, FALSE_NEW.len())])?;
    core.write_code(t, &TRUE_NEW)?;
    core.write_code(f, &FALSE_NEW)?;
    Ok(format!("at +{:#x}, +{:#x}", t - core.base, f - core.base))
}
