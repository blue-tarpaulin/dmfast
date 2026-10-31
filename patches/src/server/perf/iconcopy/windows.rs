//! `iconcopy`, Windows: the copy loop's holder construction and `DMFrameBase::init` call are rewritten in place to
//! share the source frame's holder.

use super::SPAN;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The frame loop: esi = new frame (`push 20h ; call new`) ; edi = source frame ; ebx = new holder (`push 20h ;
/// call new ; call holder ctor ; mov [ebx+1Ch],1`) ; `mov [esi],ebx ; call init`, then the assignment: copy
/// +14h..+1Ch ; if the holders differ, drop ours (count -1 -> free + delete) and share the source's (count +1) ;
/// copy +8, +4, +10h, and, if set, +0Ch.
const LOOP: &str = "6A 20 E8 ?? ?? ?? ?? 8B F0 83 C4 04 89 75 E8 C7 45 FC 00 00 00 00 85 F6 0F 84 ?? ?? ?? ?? 8B 45 08 \
                    6A 20 8B 00 8B 3C B8 E8 ?? ?? ?? ?? 8B D8 83 C4 04 89 5D E4 85 DB 74 10 8B CB E8 ?? ?? ?? ?? \
                    C7 43 1C 01 00 00 00 EB 02 33 DB 89 1E 8B CE C6 45 FC 01 E8 ?? ?? ?? ?? 3B FE 74 ?? 8B 47 14 \
                    89 46 14 8B 47 18 89 46 18 8B 47 1C 89 46 1C 8B 1E 3B 1F 74 ?? 83 43 1C FF 75 ?? FF 33 FF 15 \
                    ?? ?? ?? ?? 6A 20 53 E8 ?? ?? ?? ?? 83 C4 0C 8B 07 89 06 FF 40 1C 8B 47 08 89 46 08 8B 47 04 \
                    89 46 04 8A 47 10 88 46 10 84 C0 74 06 8B 47 0C 89 46 0C";
/// The holder ctor's and `init`'s calls.
const HOLDER_CALL: usize = 59;
const INIT_CALL: usize = 83;
/// The rewritten range: from `mov eax,[ebp+8]` up to the assignment's `cmp edi,esi`.
const FROM: usize = 0x1E;
const TO: usize = 88;
/// The assignment's `je` when the holders are equal (rel8 operand), and the `+8` copy it skips to.
const SAME_AT: usize = 115;
const SAME: usize = 148;

/// The holder ctor writes only the holder: its size from the world's icon size, malloc, memset 0.
const HOLDER: &str = "53 56 0F B7 35 ?? ?? ?? ?? 8B D9 57 0F B7 3D ?? ?? ?? ?? 6A 00 C7 03 00 00 00 00 FF 15 ?? ?? ?? ?? \
                      89 7B 04 0F AF FE 89 73 08 89 7B 0C 8D 04 BD 00 00 00 00 50 FF 15 ?? ?? ?? ?? 83 C4 08 89 03 85 C0 \
                      74 12 8B 4B 0C C1 E1 02 51 6A 00 50 E8 ?? ?? ?? ?? 83 C4 0C 5F 5E 8B C3 C6 43 18 00 5B C3";
/// `init` writes the holder (made unique, resized, zeroed) and the frame's +8 = 0, +4 = -1, +10h = 0.
const INIT: &str = "55 8B EC 51 0F B7 05 ?? ?? ?? ?? 53 0F B7 1D ?? ?? ?? ?? 56 57 8B F9 89 45 FC 8B 0F E8 ?? ?? ?? ?? \
                    8B F0 89 37 3B 5E 04 75 08 8B 45 FC 3B 46 08 74 2A FF 36 FF 15 ?? ?? ?? ?? 8B 45 FC 89 5E 04 0F AF D8 \
                    89 46 08 89 5E 0C 8D 04 9D 00 00 00 00 50 FF 15 ?? ?? ?? ?? 83 C4 08 89 06 8B 0E 85 C9 74 12 8B 46 0C \
                    C1 E0 02 50 6A 00 51 E8 ?? ?? ?? ?? 83 C4 0C C6 46 18 00 C7 47 08 00 00 00 00 C7 47 04 FF FF FF FF \
                    C6 47 10 00 5F 5E 5B 8B E5 5D C3";

/// The rewritten range, keeping the code's registers and locals: edi = source frame, ebx and [ebp-1Ch] = the frame's
/// holder, [ebp-4] = 1 (unwind state), then a short jump to the assignment and int3 padding.
fn share() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend([0x8B, 0x45, 0x08]); // mov eax,[ebp+8]
    v.extend([0x8B, 0x00]); // mov eax,[eax]
    v.extend([0x8B, 0x3C, 0xB8]); // mov edi,[eax+edi*4]
    v.extend([0x8B, 0x1F]); // mov ebx,[edi]
    v.extend([0xFF, 0x43, 0x1C]); // inc dword [ebx+1Ch]
    v.extend([0x89, 0x5D, 0xE4]); // mov [ebp-1Ch],ebx
    v.extend([0x89, 0x1E]); // mov [esi],ebx
    v.extend([0xC6, 0x45, 0xFC, 0x01]); // mov byte [ebp-4],1
    v.extend([0xC7, 0x46, 0x08, 0x00, 0x00, 0x00, 0x00]); // mov dword [esi+8],0
    v.extend([0xC7, 0x46, 0x04, 0xFF, 0xFF, 0xFF, 0xFF]); // mov dword [esi+4],-1
    v.extend([0xC6, 0x46, 0x10, 0x00]); // mov byte [esi+10h],0
    v.extend([0xEB, (TO - FROM - v.len() - 2) as u8]); // jmp assignment
    v.resize(TO - FROM, 0xCC);
    v
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let s = core.find_unique(LOOP, "icon state copy")?;
    // It must be the loop of `DMGraphic::operator=` itself, not a lookalike elsewhere.
    let f = core.export(c"??4DMGraphic@@QAEAAU0@ABU0@@Z")?;
    if !(f..f + SPAN).contains(&s) {
        bail!("icon state copy: loop at +{:#x} is not in DMGraphic::operator=", s - core.base);
    }
    if !core.is_at(core.rel32(s + HOLDER_CALL + 1)?, HOLDER)? {
        bail!("pixel holder ctor: not as expected");
    }
    if !core.is_at(core.rel32(s + INIT_CALL + 1)?, INIT)? {
        bail!("frame init: not as expected");
    }
    if core.rel8(s + SAME_AT)? != s + SAME {
        bail!("icon state copy: equal-holder branch not as expected");
    }
    core.untouched(&[(s + FROM, TO - FROM)])?;
    core.write_code(s + FROM, &share())?;
    Ok(format!("at +{:#x}", s - core.base))
}
