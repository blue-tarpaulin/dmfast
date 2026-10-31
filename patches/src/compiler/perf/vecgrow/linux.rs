//! `vecgrow`, Linux: the append's grow step (cdecl, ebx = vector, edx = size) jumps to [`grow`].

use std::arch::naked_asm;

use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

/// The append: `push ebx ; sub esp,8 ; mov ebx,[esp+10h] ; mov edx,[ebx+8] ; mov eax,[ebx] ; cmp edx,[ebx+4] ;
/// jae grow`, the store (`lea ecx,[edx+1] ; mov [ebx+8],ecx ; mov ecx,[esp+14h] ; mov [eax+edx*4],ecx ; add esp,8 ;
/// pop ebx ; ret`), padding, then the grow step: `add edx,1 ; sub esp,8 ; mov [ebx+4],edx ; shl edx,2 ; push edx ;
/// push eax ; call realloc`.
const APPEND: &str = "53 83 EC 08 8B 5C 24 10 8B 53 08 8B 03 3B 53 04 73 16 8D 4A 01 89 4B 08 8B 4C 24 14 89 0C 90 \
                      83 C4 08 5B C3 8D 74 26 00 83 C2 01 83 EC 08 89 53 04 C1 E2 02 52 50 E8";
/// The replaced `add edx,1 ; sub esp,8 ; mov [ebx+4],edx`.
const GROW: usize = 0x28;
const GROW_LEN: usize = 9;

/// The `shl edx,2` after the grow step.
static BACK: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// The grow step with the new capacity `2 * size + 4`.
///
/// # Safety
/// Only jumped to from the append.
#[unsafe(naked)]
unsafe extern "C" fn grow() {
    naked_asm!(
        "lea edx, [edx + edx + 4]",
        "sub esp, 8",
        "mov dword ptr [ebx + 4], edx",
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
