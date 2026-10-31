//! `nulldiv`, Windows: the `/` jump table's null entry goes to [`null_div`]. The shared "result is 0" block it
//! used is left alone: other operators use it.

use std::arch::naked_asm;

use super::{NULL, NUMBER};
use crate::base::Result;
use crate::base::code::{Foreign, hx};
use crate::base::feature::Ctx;

/// The null case: `push ecx ; mov dword [esp],0 ; jmp finish`, then the next case's start.
const NULL_CASE: &str = "51 C7 04 24 00 00 00 00 E9 ?? ?? ?? ?? FF B5 E4 FD FF FF FF B5 E0 FD FF FF E8 ?? ?? ?? ?? \
                         8B 3D ?? ?? ?? ?? 83 C4 08 D9 9D DC FD FF FF";
/// Its `jmp finish`.
const FINISH_AT: usize = 8;
/// The number case: `cmp byte [right type],2Ah ; jne ; movss xmm1,[right value] ; ucomiss xmm1,[0.0] ; lahf ;
/// test ah,44h ; jnp div_by_zero ; movss xmm0,[left] ; divss xmm0,xmm1`.
const NUMBER_CASE: &str = "80 BD E0 FD FF FF 2A 0F 85 ?? ?? ?? ?? F3 0F 10 8D B0 FD FF FF 0F 2E 0D ?? ?? ?? ?? 9F F6 C4 44 \
                           0F 8B ?? ?? ?? ?? F3 0F 10 44 CA FC F3 0F 5E C1";
/// Its `jnp div_by_zero`.
const ERROR_AT: usize = 32;
/// The right operand's type and value in the handler's frame (literals in [`NUMBER_CASE`]).
const RIGHT_TYPE: i32 = -0x220;
const RIGHT_VALUE: i32 = -0x250;

/// The "Division by zero" error and the null case's original continuation.
static ERROR: Foreign<unsafe extern "C" fn()> = Foreign::new();
static FINISH: Foreign<unsafe extern "C" fn()> = Foreign::new();
static ZERO: f32 = 0.0;

/// The null case: a number right operand goes through the number case's zero check, null is a division by zero,
/// anything else runs stock's null case.
///
/// # Safety
/// Only jumped to from the `/` handler's jump table.
#[unsafe(naked)]
unsafe extern "C" fn null_div() {
    naked_asm!(
        "cmp byte ptr [ebp + {ty}], {number}",
        "je 2f",
        "cmp byte ptr [ebp + {ty}], {null}",
        "jne 3f",
        "jmp dword ptr [{error}]",
        "2:",
        "movss xmm1, dword ptr [ebp + {value}]",
        "ucomiss xmm1, dword ptr [{zero}]",
        // Equal and ordered: zero.
        "jp 3f",
        "jne 3f",
        "jmp dword ptr [{error}]",
        "3:",
        "push ecx",
        "mov dword ptr [esp], 0",
        "jmp dword ptr [{finish}]",
        ty = const RIGHT_TYPE,
        value = const RIGHT_VALUE,
        number = const NUMBER,
        null = const NULL,
        zero = sym ZERO,
        error = sym ERROR,
        finish = sym FINISH,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let null = core.find_unique(NULL_CASE, "null / x case")?;
    let num = core.find_unique(NUMBER_CASE, "number / x case")?;
    // The `/` jump table on the left type: entry 0 is the null case, entry 3 the number case.
    let table = core.find_unique(&format!("{} ?? ?? ?? ?? ?? ?? ?? ?? {}", hx(null), hx(num)), "/ jump table")?;
    // Its dispatch is `movzx eax,byte [eax+index] ; jmp [eax*4+table]`: the index must map null to entry 0 and a
    // number to entry 3.
    let dispatch = core.find_unique(&format!("0F B6 80 ?? ?? ?? ?? FF 24 85 {}", hx(table)), "/ dispatch")?;
    let index = core.ptr_at(dispatch + 3)?;
    core.expect(index + usize::from(NULL), &[0], "/ dispatch: null does not lead to the null case")?;
    core.expect(index + usize::from(NUMBER), &[3], "/ dispatch: a number does not lead to the number case")?;
    ERROR.set(core.rel32(num + ERROR_AT + 2)?);
    FINISH.set(core.rel32(null + FINISH_AT + 1)?);
    core.write_code(table, &(null_div as unsafe extern "C" fn() as usize as u32).to_le_bytes())?;
    Ok(format!("at +{:#x}", table - core.base))
}
