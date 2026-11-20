//! `callleak`, Windows: each call_ext() opcode's buffer is a local of the interpreter's frame (`lea eax,[ebp-x]`).

use crate::base::code::Foreign;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The buffer's allocation: push ebp ; mov ebp,esp ; mov ecx,[ebp+8] (the buffer) ; mov eax,[ebp+0Ch] (the count) ;
/// push esi ; mov edx,[ecx+used] ; lea esi,[edx+eax] ; cmp esi,inline slots ; jbe inline ; and eax,-8 ; add eax,8 ;
/// mov [ecx+block],eax ; shl eax,3 ; push eax ; call [malloc]
pub(super) const ALLOC: &str = "55 8B EC 8B 4D 08 8B 45 0C 56 8B 51 ?? 8D 34 02 83 FE ?? 76 ?? 83 E0 F8 83 C0 08 89 41 \
                                ?? C1 E0 03 50 FF 15";
/// Its `used` and `block` offsets.
pub(super) const USED: usize = 12;
pub(super) const BLOCK: usize = 29;
/// Its `malloc` import slot.
const MALLOC: usize = 36;
/// An opcode's allocation: lea eax,[ebp-x] (the buffer) ; push esi (the count) ; push eax ; call alloc
pub(super) const ALLOC_CALL: &str = "8D 85 ?? ?? ?? ?? 56 50 E8";
/// The buffer's cleanup, after the call and in the handler for a throwing call: lea eax,[ebp-x] ; (add esp,8) ;
/// push eax ; call cleanup
pub(super) const CLEANUP: [&str; 2] = ["8D 85 ?? ?? ?? ?? 50 E8", "8D 85 ?? ?? ?? ?? 83 C4 ?? 50 E8"];

/// Whether code starts with a bare return (`ret`, `ret 0`).
pub(super) fn is_ret(code: &[u8]) -> bool {
    matches!(code, [0xC3, ..] | [0xC2, 0, 0, ..])
}

/// Checks the allocation takes its block from the `malloc` BYOND imports, and returns that runtime's `free`.
pub(super) fn free(core: &Image, alloc: usize) -> Result<Foreign<unsafe extern "C" fn(usize)>> {
    ensure!(core.ptr_at(alloc + MALLOC)? == core.iat_slot("malloc")?, "call_ext(arglist()): the buffer doesn't malloc");
    Ok(Foreign::at(core.live_ptr(core.iat_slot("free")?)?))
}
