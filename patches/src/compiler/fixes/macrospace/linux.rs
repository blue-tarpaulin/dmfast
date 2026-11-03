//! `macrospace`, Linux: the text stream's reader (cdecl, slot 2 of its vtable) jumps at entry to [`reader`].

use std::arch::naked_asm;

use super::{CURRENT, FOUND, Found, POS, TEXT, drop_space};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::linux::glibc::{self, File};
use crate::base::{Result, bail, ensure};

/// The text stream's reader: mov eax,[esp+4] ; mov edx,[eax+3Ch] ; mov ecx,[eax+34h] ; movzx ecx,byte [ecx+edx] ;
/// mov [eax+40h],ecx ; test ecx,ecx ; je end ; add edx,1 ; mov [eax+3Ch],edx ; mov eax,ecx ; ret ; ...
/// end: mov ecx,-1 ; jmp ret
const READER: &str = "8B 44 24 04 8B 50 3C 8B 48 34 0F B6 0C 11 89 48 40 85 C9 74 0B 83 C2 01 89 50 3C 89 C8 C3 66 90 \
                      B9 FF FF FF FF EB F4";
/// The hooked bytes.
const ENTRY: usize = 8;
/// The text stream's constructor: mov eax,[ebp+10h] ; mov [ebx+30h],0 ; mov [ebx+34h],eax (text) ; mov eax,[ebp+14h] ;
/// mov dword [ebx],vtable ; mov [ebx+PARENT],eax ; ... ; movzx eax,byte [ebp-1Ch] ; mov [ebx+KIND],al ;
/// mov eax,edi ; mov [ebx+38h],al
const CTOR: &str = "8B 45 10 C7 43 30 00 00 00 00 89 43 34 8B 45 14 C7 03 ?? ?? ?? ?? 89 43 ?? 8B 45 18 C7 43 3C 00 00 \
                    00 00 89 43 1C 8B 45 1C C6 43 2E 00 89 43 14 0F B6 45 E4 88 43 ?? 89 F8 88 43 38";
const VTABLE: usize = 18;
const PARENT: usize = 24;
const KIND: usize = 54;
/// Before that it stores the base class's vtable (the file stream's): lea esi,[ebx+18h] ; mov dword [ebx],vtable.
const BASE: &str = "8D 73 18 C7 03 ?? ?? ?? ??";
/// The file stream's reader: mov eax,[esp+4] ; mov eax,[eax+8] ; mov [esp+4],eax ; jmp getc (or stdio's).
const FILE_READER: &str = "8B 44 24 04 8B 40 08 89 44 24 04 E9";
/// Pushing a stream onto the lexer (esi), inlined at several sites, which must agree: mov eax,[esi+24h] ; ... ;
/// test eax,eax ; je ; mov edx,[esi+18h] ; mov [eax+SAVED],edx ; [sub esp,0Ch] ; mov [esi+24h],ebx.
const PUSHES: [&str; 2] = ["85 C0 74 06 8B 56 18 89 50 ?? 89 5E 24", "85 C0 74 06 8B 56 18 89 50 ?? 83 EC 0C 89 5E 24"];
const SAVED: usize = 9;
/// The reader's vtable slot.
const SLOT: usize = 2;

/// The text stream's reader (cdecl: stream) with one change: a space first asks [`drop_space`], and a dropped one reads
/// as the end of the text. The call is 16-byte aligned whatever the caller's alignment.
///
/// # Safety
/// Only jumped to from the reader's entry.
#[unsafe(naked)]
unsafe extern "C" fn reader() {
    naked_asm!(
        "mov eax, dword ptr [esp + 4]",
        "mov edx, dword ptr [eax + {pos}]",
        "mov ecx, dword ptr [eax + {text}]",
        "movzx ecx, byte ptr [ecx + edx]",
        "cmp ecx, 0x20",
        "je 3f",
        "2:",
        "mov dword ptr [eax + {current}], ecx",
        "test ecx, ecx",
        "je 4f",
        "add edx, 1",
        "mov dword ptr [eax + {pos}], edx",
        "mov eax, ecx",
        "ret",
        "4:",
        "mov eax, -1",
        "ret",
        // A space: keep the stream (eax) and ebx across the call.
        "3:",
        "push eax",
        "push ebx",
        "mov ebx, esp",
        "sub esp, 16",
        "and esp, -16",
        "mov dword ptr [esp], eax",
        "call {drop_space}",
        "mov esp, ebx",
        "pop ebx",
        "mov ecx, eax",
        "pop eax",
        "test ecx, ecx",
        "jne 5f",
        "mov edx, dword ptr [eax + {pos}]",
        "mov ecx, 0x20",
        "jmp 2b",
        // Dropped: the end of the text.
        "5:",
        "mov dword ptr [eax + {current}], 0",
        "mov eax, -1",
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
    let ctor = core.find_unique(CTOR, "text stream constructor")?;
    ensure!(
        core.ptr_at(core.ptr_at(ctor + VTABLE)? + 4 * SLOT)? == g,
        "text stream constructor: not the reader's class"
    );
    let base = core.find_unique_in(ctor - 0x60, 0x60, BASE, "text stream constructor: base vtable")?;
    let file_reader = core.ptr_at(core.ptr_at(base + 5)? + 4 * SLOT)?;
    core.require(file_reader, FILE_READER, "file stream reader")?;
    let mut saved = Vec::new();
    for pattern in PUSHES {
        for s in core.find_all(pattern)? {
            let b = core.u8_at(s + SAVED)?;
            saved.push(b);
        }
    }
    let Some(&first) = saved.first().filter(|&&o| saved.iter().all(|&s| s == o)) else {
        bail!("stream push: {} sites, not all with the same saved character offset", saved.len());
    };
    let parent = core.u8_at(ctor + PARENT)?;
    let kind = core.u8_at(ctor + KIND)?;
    let found = Found {
        parent: usize::from(parent),
        kind: usize::from(kind),
        saved: usize::from(first),
        slot: SLOT,
        text_reader: g,
        file_reader,
        getc: Foreign::at(glibc::getc as unsafe extern "C" fn(*mut File) -> i32),
        ungetc: Foreign::at(glibc::ungetc as unsafe extern "C" fn(i32, *mut File) -> i32),
    };
    core.untouched(&[(g, ENTRY)])?;
    core.jump(g, ENTRY, reader as unsafe extern "C" fn())?;
    FOUND.commit(cx, found);
    Ok(core.report(&[g]))
}
