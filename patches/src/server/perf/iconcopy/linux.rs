//! `iconcopy`, Linux: the copy loop's `DMFrameBase::DMFrameBase()` call calls [`share`] instead.

use std::arch::naked_asm;

use super::SPAN;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The frame loop: `push 20h ; call new` (ebx = the frame) ; esi = the source frame ; `mov [esp],ebx ;
/// call DMFrameBase()`, then the assignment: copy +14h..+1Ch, and if the holders differ drop ours (count -1).
const LOOP: &str = "83 EC 0C 6A 20 E8 ?? ?? ?? ?? 89 C3 8D 04 BD 00 00 00 00 89 45 E4 8B 45 0C 8B 00 8B 34 B8 89 1C 24 \
                    E8 ?? ?? ?? ?? 83 C4 10 39 DE 74 ?? D9 46 1C F3 0F 7E 46 14 66 0F D6 43 14 D9 5B 1C 39 F3 74 ?? \
                    8B 03 3B 06 74 ?? 83 68 1C 01";
/// Its `call DMFrameBase()`.
const CALL: usize = 33;
/// Its `je same` when the holders are equal (rel8 operand).
const SAME_AT: usize = 70;
/// `DMFrameBase()`: a new holder of the icon size (calloc'd, count 1), then frame +4 = -1, +8 = 0, +10h = 0.
/// Nothing else.
const CTOR: &str = "56 53 83 EC 10 8B 74 24 1C 6A 20 E8 ?? ?? ?? ?? 0F B7 15 ?? ?? ?? ?? 89 C3 0F B7 05 ?? ?? ?? ?? \
                    66 0F 6E C2 66 0F 6E C8 0F AF C2 5A 59 6A 01 66 0F 62 C1 66 0F D6 43 04 89 43 0C C1 E0 02 50 \
                    E8 ?? ?? ?? ?? 83 C4 10 31 D2 89 1E 89 03 B8 FF FF FF FF 89 46 04 89 56 08 C6 46 10 00 C6 43 18 00 \
                    C7 43 1C 01 00 00 00 83 C4 04 5B 5E C3";
/// The equal-holder path: copy +8, +4, +10h, and, if set, +0Ch (as after sharing).
const SAME: &str = "8B 46 08 89 43 08 8B 46 04 89 43 04 0F B6 46 10 88 43 10 84 C0 74 06 8B 46 0C 89 43 0C";

/// `DMFrameBase()` on the new frame ([esp+4]), but on esi's (the source frame's) holder.
///
/// # Safety
/// Only called from the copy loop.
#[unsafe(naked)]
unsafe extern "C" fn share() {
    naked_asm!(
        "mov eax, dword ptr [esp + 4]",
        "mov edx, dword ptr [esi]",
        "add dword ptr [edx + 0x1C], 1",
        "mov dword ptr [eax], edx",
        "mov dword ptr [eax + 4], -1",
        "mov dword ptr [eax + 8], 0",
        "mov byte ptr [eax + 0x10], 0",
        "ret",
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let s = core.find_unique(LOOP, "icon state copy")?;
    // It must be the loop of `DMGraphic::operator=(DMGraphic const&)` itself, not a lookalike elsewhere.
    let f = core.export(c"_ZN9DMGraphicaSERKS_")?;
    if !(f..f + SPAN).contains(&s) {
        bail!("icon state copy: loop at +{:#x} is not in DMGraphic::operator=", s - core.base);
    }
    if !core.is_at(core.rel32(s + CALL + 1)?, CTOR)? {
        bail!("frame constructor: not as expected");
    }
    if !core.is_at(core.rel8(s + SAME_AT)?, SAME)? {
        bail!("icon state copy: equal-holder path not as expected");
    }
    core.call(s + CALL, 5, share as unsafe extern "C" fn())?;
    Ok(format!("at +{:#x}", s + CALL - core.base))
}
