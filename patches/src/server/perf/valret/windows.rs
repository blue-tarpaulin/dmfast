//! `valret`, Windows: each `Value` maker's body after its frame setup is rewritten in place.

use crate::base::code::pad;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// An id, or null for 0xFFFF (cdecl: id, type): push ebp ; mov ebp,esp ; sub esp,8 ; movzx eax,byte [ebp+0Ch] ;
/// xor ecx,ecx ; push esi ; mov esi,[ebp+8] ; cmp esi,0FFFFh ; cmovne ecx,eax ; xor edx,edx ; cmp esi,0FFFFh ;
/// mov [ebp-8],cl ; mov eax,[ebp-8] ; cmovne edx,esi ; pop esi ; mov esp,ebp ; pop ebp ; ret
const ID_OR_NULL: &str = "55 8B EC 83 EC 08 0F B6 45 0C 33 C9 56 8B 75 08 81 FE FF FF 00 00 0F 45 C8 33 D2 81 FE FF FF 00 00 \
                          88 4D F8 8B 45 F8 0F 45 D6 5E 8B E5 5D C3";
/// A number (cdecl: float): push ebp ; mov ebp,esp ; sub esp,8 ; movss xmm0,[ebp+8] ; mov byte [ebp-8],2Ah ;
/// mov eax,[ebp-8] ; movss [ebp-4],xmm0 ; mov edx,[ebp-4] ; mov esp,ebp ; pop ebp ; ret
const NUMBER: &str = "55 8B EC 83 EC 08 F3 0F 10 45 08 C6 45 F8 2A 8B 45 F8 F3 0F 11 45 FC 8B 55 FC 8B E5 5D C3";
/// An id of the type given (cdecl: id, type): push ebp ; mov ebp,esp ; sub esp,8 ; mov al,[ebp+0Ch] ;
/// mov edx,[ebp+8] ; mov [ebp-8],al ; mov eax,[ebp-8] ; mov esp,ebp ; pop ebp ; ret
const ID: &str = "55 8B EC 83 EC 08 8A 45 0C 8B 55 08 88 45 F8 8B 45 F8 8B E5 5D C3";
/// An id of one fixed type (cdecl: id; several functions): push ebp ; mov ebp,esp ; sub esp,8 ; mov edx,[ebp+8] ;
/// mov byte [ebp-8],type ; mov eax,[ebp-8] ; mov esp,ebp ; pop ebp ; ret
const FIXED: &str = "55 8B EC 83 EC 08 8B 55 08 C6 45 F8 ?? 8B 45 F8 8B E5 5D C3";
/// Where [`FIXED`]'s type byte is.
const FIXED_TYPE: usize = 12;
/// An id of the type given, null for 0xFFFF unless the type keeps it (cdecl: id, type): push ebp ; mov ebp,esp ;
/// mov cl,[ebp+0Ch] ; sub esp,8 ; movzx eax,cl ; dec eax ; cmp eax,4Dh ; ja null_check ; movzx eax,byte [eax+INDEX] ;
/// jmp [eax*4+TARGETS] ; keep: mov edx,[ebp+8] ; TAIL ; null_check: mov edx,[ebp+8] ; cmp edx,0FFFFh ; jne tail ;
/// xor cl,cl ; xor edx,edx ; tail: mov [ebp-8],cl ; mov eax,[ebp-8] ; mov esp,ebp ; pop ebp ; ret
const TYPED: &str = "55 8B EC 8A 4D 0C 83 EC 08 0F B6 C1 48 83 F8 4D 77 1B 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ?? \
                     8B 55 08 88 4D F8 8B 45 F8 8B E5 5D C3 8B 55 08 81 FA FF FF 00 00 75 04 32 C9 33 D2 \
                     88 4D F8 8B 45 F8 8B E5 5D C3";
/// [`TYPED`]'s index table, last index, jump table, and its two cases and tail.
const TYPED_INDEX: usize = 0x15;
const TYPED_LAST: usize = 0x4D;
const TYPED_TARGETS: usize = 0x1C;
const TYPED_KEEP: usize = 0x20;
const TYPED_NULL_CHECK: usize = 0x2D;
const TYPED_TAIL: usize = 0x3C;
/// Each maker's frame setup (push ebp ; mov ebp,esp ; sub esp,8), kept.
const FRAME: usize = 6;

#[rustfmt::skip]
const ID_OR_NULL_NEW: [u8; 41] = pad(&[
    0x8B, 0x45, 0xF8,                   // mov eax,[ebp-8]
    0x0F, 0xB6, 0x4D, 0x0C,             // movzx ecx,byte [ebp+0Ch]
    0x56,                               // push esi
    0x8B, 0x75, 0x08,                   // mov esi,[ebp+8]
    0x33, 0xD2,                         // xor edx,edx
    0x81, 0xFE, 0xFF, 0xFF, 0x00, 0x00, // cmp esi,0FFFFh
    0x0F, 0x44, 0xCA,                   // cmove ecx,edx
    0x0F, 0x45, 0xD6,                   // cmovne edx,esi
    0x8A, 0xC1,                         // mov al,cl
    0x88, 0x4D, 0xF8,                   // mov [ebp-8],cl
    0x5E,                               // pop esi
    0x8B, 0xE5,                         // mov esp,ebp
    0x5D,                               // pop ebp
    0xC3,                               // ret
]);

#[rustfmt::skip]
const NUMBER_NEW: [u8; 24] = pad(&[
    0x8B, 0x45, 0xF8,             // mov eax,[ebp-8]
    0xF3, 0x0F, 0x10, 0x45, 0x08, // movss xmm0,[ebp+8]
    0x8B, 0x55, 0x08,             // mov edx,[ebp+8]
    0xB0, 0x2A,                   // mov al,2Ah
    0x88, 0x45, 0xF8,             // mov [ebp-8],al
    0x89, 0x55, 0xFC,             // mov [ebp-4],edx
    0x8B, 0xE5,                   // mov esp,ebp
    0x5D,                         // pop ebp
    0xC3,                         // ret
]);

#[rustfmt::skip]
const ID_NEW: [u8; 12] = [
    0x8B, 0x45, 0xF8, // mov eax,[ebp-8]
    0x8A, 0x45, 0x0C, // mov al,[ebp+0Ch]
    0x8B, 0x55, 0x08, // mov edx,[ebp+8]
    0x88, 0x45, 0xF8, // mov [ebp-8],al
];

/// The type goes in at [`FIXED_NEW_TYPE`].
#[rustfmt::skip]
const FIXED_NEW: [u8; 14] = pad(&[
    0x8B, 0x45, 0xF8, // mov eax,[ebp-8]
    0x8B, 0x55, 0x08, // mov edx,[ebp+8]
    0xB0, 0x00,       // mov al,type
    0x88, 0x45, 0xF8, // mov [ebp-8],al
    0xC9,             // leave
    0xC3,             // ret
]);
const FIXED_NEW_TYPE: usize = 7;

#[rustfmt::skip]
const TYPED_KEEP_NEW: [u8; 13] = [
    0x8B, 0x45, 0xF8, // mov eax,[ebp-8]
    0x8A, 0xC1,       // mov al,cl
    0x8B, 0x55, 0x08, // mov edx,[ebp+8]
    0x88, 0x4D, 0xF8, // mov [ebp-8],cl
    0xC9,             // leave
    0xC3,             // ret
];

#[rustfmt::skip]
const TYPED_TAIL_NEW: [u8; 10] = [
    0x8B, 0x45, 0xF8, // mov eax,[ebp-8]
    0x8A, 0xC1,       // mov al,cl
    0x88, 0x4D, 0xF8, // mov [ebp-8],cl
    0xC9,             // leave
    0xC3,             // ret
];

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let id_or_null = core.find_unique(ID_OR_NULL, "id-or-null Value")?;
    let number = core.find_unique(NUMBER, "number Value")?;
    let id = core.find_unique(ID, "id Value")?;
    let typed = core.find_unique(TYPED, "typed id Value")?;
    let fixed = core.find_all(FIXED)?;
    ensure!(!fixed.is_empty(), "fixed-type Value: not found");
    // The typed maker's computed jump must reach only its two cases.
    let (index, targets) = (core.ptr_at(typed + TYPED_INDEX)?, core.ptr_at(typed + TYPED_TARGETS)?);
    for t in 0..=TYPED_LAST {
        ensure!(core.u8_at(index + t)? <= 1, "typed id Value: a third case");
    }
    ensure!(
        core.ptr_at(targets)? == typed + TYPED_KEEP && core.ptr_at(targets + 4)? == typed + TYPED_NULL_CHECK,
        "typed id Value: cases not as expected"
    );

    let mut writes = vec![
        (id_or_null + FRAME, ID_OR_NULL_NEW.to_vec()),
        (number + FRAME, NUMBER_NEW.to_vec()),
        (id + FRAME, ID_NEW.to_vec()),
        (typed + TYPED_KEEP, TYPED_KEEP_NEW.to_vec()),
        (typed + TYPED_TAIL, TYPED_TAIL_NEW.to_vec()),
    ];
    for &f in fixed.iter() {
        let mut code = FIXED_NEW;
        code[FIXED_NEW_TYPE] = core.u8_at(f + FIXED_TYPE)?;
        writes.push((f + FRAME, code.to_vec()));
    }
    let ranges: Vec<(usize, usize)> = writes.iter().map(|(at, code)| (*at, code.len())).collect();
    core.untouched(&ranges)?;
    for (at, code) in &writes {
        core.write_code(*at, code)?;
    }
    Ok(format!(
        "id or null +{:#x}, number +{:#x}, id +{:#x}, typed id +{:#x}, {} fixed-type",
        id_or_null - core.base,
        number - core.base,
        id - core.base,
        typed - core.base,
        fixed.len()
    ))
}
