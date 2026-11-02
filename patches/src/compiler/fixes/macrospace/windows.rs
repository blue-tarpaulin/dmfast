//! `macrospace`, Windows: the text stream's reader (thiscall, slot 1 of its vtable) jumps at entry to [`reader`].

use std::arch::naked_asm;

use super::{CURRENT, FOUND, Found, POS, TEXT, drop_space};
use crate::base::code::{Foreign, hx};
use crate::base::feature::Ctx;
use crate::base::windows::ucrt::ucrt_export;
use crate::base::{Result, bail};

/// The text stream's reader: `mov edx,ecx ; mov ecx,[edx+3Ch] ; mov eax,[edx+34h] ; movzx eax,byte [eax+ecx] ;
/// mov [edx+40h],eax ; test eax,eax ; jne ok ; or eax,-1 ; ret ; ok: inc ecx ; mov [edx+3Ch],ecx ; ret`.
const READER: &str = "8B D1 8B 4A 3C 8B 42 34 0F B6 04 08 89 42 40 85 C0 75 04 83 C8 FF C3 41 89 4A 3C C3";
/// The hooked bytes.
const ENTRY: usize = 5;
/// The file stream's reader (the other one enclosing an expansion): `push [ecx+8] ; call [getc] ; add esp,4 ; ret`,
/// matched from the getc operand on (the stdio patch may replace the first 5 bytes).
const FILE_READER: &str = "{getc} 83 C4 04 C3";
/// Pushing a macro expansion (lexer in ebx): mov esi,[ebx+24h] (the lexer's current stream) ; mov ecx,edi ; push ebx ;
/// call base_ctor ; mov eax,[ebp+8] ; mov [edi+34h],eax (text) ; ... ; mov al,[ebp+0Ch] ; mov [edi+KIND],al ;
/// mov al,[ebp+10h] ; mov dword [edi],vtable ; mov [edi+PARENT],esi
const PUSH: &str = "8B 73 24 8B CF 53 E8 ?? ?? ?? ?? 8B 45 08 89 47 34 8B 45 14 89 47 1C 8B 45 18 89 47 14 8A 45 0C 88 \
                    47 ?? 8A 45 10 C7 07 ?? ?? ?? ?? 89 77 ??";
const KIND: usize = 34;
const VTABLE: usize = 40;
const PARENT: usize = 46;
/// Then the expansion becomes the lexer's current stream: inc [ebx+28h] ; mov ecx,[ebx+24h] ; test ecx,ecx ; je ;
/// mov eax,[ebx+18h] ; mov [ecx+SAVED],eax: the covered stream keeps the lexer's current character.
const SAVE: &str = "FF 43 28 8B 4B 24 85 C9 74 06 8B 43 18 89 41 ??";
const SAVED: usize = 15;
/// The reader's vtable slot.
const SLOT: usize = 1;

/// The text stream's reader (thiscall, ecx = stream) with one change: a space first asks [`drop_space`], and a dropped
/// one reads as the end of the text.
///
/// # Safety
/// Only jumped to from the reader's entry.
#[unsafe(naked)]
unsafe extern "C" fn reader() {
    naked_asm!(
        "mov edx, ecx",
        "mov ecx, dword ptr [edx + {pos}]",
        "mov eax, dword ptr [edx + {text}]",
        "movzx eax, byte ptr [eax + ecx]",
        "cmp al, 0x20",
        "je 3f",
        "2:",
        "mov dword ptr [edx + {current}], eax",
        "test eax, eax",
        "jne 4f",
        "or eax, -1",
        "ret",
        "4:",
        "inc ecx",
        "mov dword ptr [edx + {pos}], ecx",
        "ret",
        // A space: keep ecx and edx across the call.
        "3:",
        "push ecx",
        "push edx",
        "push edx",
        "call {drop_space}",
        "add esp, 4",
        "test eax, eax",
        "pop edx",
        "pop ecx",
        "jne 5f",
        "mov eax, 0x20",
        "jmp 2b",
        // Dropped: the end of the text.
        "5:",
        "mov dword ptr [edx + {current}], 0",
        "or eax, -1",
        "ret",
        drop_space = sym drop_space,
        text = const TEXT,
        pos = const POS,
        current = const CURRENT,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let g = core.find_unique(READER, "text stream reader")?;
    let pattern = FILE_READER.replace("{getc}", &hx(core.ucrt_slot("getc")?));
    let file_reader = core.find_unique(&pattern, "file stream reader")? - 5;
    let push = core.find_unique(PUSH, "macro text push")?;
    // It builds the hooked reader's class.
    if core.ptr_at(core.ptr_at(push + VTABLE)? + 4 * SLOT)? != g {
        bail!("macro text push: not the text stream reader's class");
    }
    let save = core.find_unique_in(push, 0x100, SAVE, "macro text push: current character save")?;
    let parent = core.u8_at(push + PARENT)?;
    let kind = core.u8_at(push + KIND)?;
    let saved = core.u8_at(save + SAVED)?;
    let found = Found {
        parent: usize::from(parent),
        kind: usize::from(kind),
        saved: usize::from(saved),
        slot: SLOT,
        text_reader: g,
        file_reader,
        getc: Foreign::at(ucrt_export("getc")?),
        ungetc: Foreign::at(ucrt_export("ungetc")?),
    };
    core.untouched(&[(g, ENTRY)])?;
    core.jump(g, ENTRY, reader as unsafe extern "C" fn())?;
    FOUND.commit(cx, found);
    Ok(format!("at +{:#x}", g - core.base))
}
