//! `shiftassign`, Windows: right after its overload call, the shift-assign overload case jumps to `+=`'s, which
//! takes the result, lets go of the old value and writes the result.

use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// `+=`'s overload case: push 0 ; push 3Eh (`+`) ; push 45h (`+=`) ; push edx ; push esi ; call find_overload
const ADD_CASE: &str = "6A 00 6A 3E 6A 45 52 56 E8";
/// Right after its `call overload` (which `overloadcatch` retargets, so not part of the patterns): add esp,24h ;
/// (the result, edx:eax, replaces the operand) ; push esi ; call release (the old value) ; add esp,8 ;
/// mov eax,[ebp-24Ch] (the opcode's position) ; push eax ; (the write)
const ADD_RESULT: &str = "83 C4 24 89 95 ?? ?? ?? ?? 89 85 ?? ?? ?? ?? 8B 8D ?? ?? ?? ?? 8B B5 ?? ?? ?? ?? 89 95 ?? ?? ?? \
                          ?? 51 89 85 ?? ?? ?? ?? 56 E8 ?? ?? ?? ?? 83 C4 08 8B 85 ?? ?? ?? ?? 50";
/// `<<=`: push 0 ; push 82h (`<<`) ; push 4Dh (`<<=`) ; push the operand ; call find_overload ; mov edi,eax ;
/// add esp,14h ; cmp edi,0FFFFh ; je no_overload ; then the overload case
const SHL: &str = "6A 00 68 82 00 00 00 6A 4D FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 8B F8 83 C4 14 81 FF \
                   FF FF 00 00 0F 84 ?? ?? ?? ??";
/// `>>=`: the same with 83h and 4Eh, and jne overload_case (`<<=`'s)
const SHR: &str = "6A 00 68 83 00 00 00 6A 4E FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 8B F8 83 C4 14 81 FF \
                   FF FF 00 00 0F 85";
/// The overload case right after its `call overload`: mov esi,[ebp-218h] ; add esp,24h ; (the result replaces the
/// operand) ; push ecx ; push esi ; call release (the old value) ; mov edi,[state] ; add esp,8 ;
/// inc word [edi+14h] ; jmp loop. Other opcodes share the code from the `push ecx` on, so the jump to `+=`'s case
/// replaces the `mov esi`.
const SHIFT_RESULT: &str = "8B B5 ?? ?? ?? ?? 83 C4 24 8B 8D ?? ?? ?? ?? 89 85 ?? ?? ?? ?? 89 95 ?? ?? ?? ?? 89 85 ?? ?? \
                            ?? ?? 89 95 ?? ?? ?? ?? 51 56 E8 ?? ?? ?? ?? 8B 3D ?? ?? ?? ?? 83 C4 08 66 FF 47 14 E9";

/// The function the `call` that ends right before `at` called as shipped, not relied on.
fn call_before(core: &Image, at: usize) -> Result<usize> {
    let b = core.haystack(at - 5, 5)?;
    ensure!(b[0] == 0xE8, "{at:#x}: no call before");
    Ok(at.wrapping_add(i32::from_le_bytes([b[1], b[2], b[3], b[4]]) as usize))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let add = core.find_unique(ADD_CASE, "+= overload case")?;
    let add_result = core.find_unique_in(add, 0x80, ADD_RESULT, "+= overload case: result")?;
    let shl = core.find_unique(SHL, "<<= opcode")?;
    let case = shl + SHL.split_whitespace().count();
    let shr = core.find_unique(SHR, ">>= opcode")?;
    ensure!(core.rel32(shr + SHR.split_whitespace().count())? == case, ">>= doesn't share <<='s overload case");
    let result = core.find_unique_in(case, 0x100, SHIFT_RESULT, "shift-assign overload case: result")?;
    ensure!(
        call_before(core, result)? == call_before(core, add_result)?,
        "shift-assign overload case: another overload call"
    );
    core.jump(result, 6, add_result)?;
    Ok(core.report(&[result]))
}
