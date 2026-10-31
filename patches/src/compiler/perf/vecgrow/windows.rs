//! `vecgrow`, Windows: the append's grow step (thiscall, esi = vector, edx = size) jumps to [`grow`].

use std::arch::naked_asm;

use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

/// The append: `push ebp ; mov ebp,esp ; push esi ; mov esi,ecx ; mov edx,[esi+8] ; cmp edx,[esi+4] ; jb store`,
/// then the grow step: `lea eax,[edx+1] ; mov [esi+4],eax ; shl eax,2 ; push eax ; push [esi] ; call [realloc]`.
const APPEND: &str = "55 8B EC 56 8B F1 8B 56 08 3B 56 04 72 1A 8D 42 01 89 46 04 C1 E0 02 50 FF 36 FF 15";
/// The replaced `lea eax,[edx+1] ; mov [esi+4],eax ; shl eax,2`.
const GROW: usize = 14;
const GROW_LEN: usize = 9;

/// The `push eax` after the grow step.
static BACK: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// The grow step with the new capacity `2 * size + 4`.
///
/// # Safety
/// Only jumped to from the append.
#[unsafe(naked)]
unsafe extern "C" fn grow() {
    naked_asm!(
        "lea eax, [edx + edx + 4]",
        "mov dword ptr [esi + 4], eax",
        "shl eax, 2",
        "jmp dword ptr [{back}]",
        back = sym BACK,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(APPEND, "dword-vector append")?;
    core.untouched(&[(f + GROW, GROW_LEN)])?;
    BACK.set(f + GROW + GROW_LEN);
    core.jump(f + GROW, GROW_LEN, grow as unsafe extern "C" fn())?;
    Ok(format!("at +{:#x}", f - core.base))
}
