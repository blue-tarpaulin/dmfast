//! `children`, Windows: the scan's entry jumps to [`scan_stub`].

use std::arch::naked_asm;

use super::{init, lookup};
use crate::base::code::Addr;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::layout::COUNT;

/// The scan: movzx edi,word [esi+22h] ; xor eax,eax ; movzx edx,cx ; xor ecx,ecx ; cmp ax,di ; jae miss ;
/// mov ebx,[esi+1Ch] ; nop2 ; loop: movzx eax,cx ; mov esi,[ebx+eax*4] ; mov eax,[ebp-14h] ; cmp [esi],eax ;
/// jne next ; mov ax,dx ; add edx,0FFFFh ; test ax,ax ; je found ; next: inc ecx ; cmp cx,di ; jb loop ;
/// jmp miss ; found: mov ebx,[ebp-14h] ; mov edi,[ebp+8]
const SCAN: &str = "0F B7 7E 22 33 C0 0F B7 D1 33 C9 66 3B C7 0F 83 ?? ?? ?? ?? 8B 5E 1C 66 90 0F B7 C1 8B 34 83 \
                    8B 45 EC 39 06 75 0E 66 8B C2 81 C2 FF FF 00 00 66 85 C0 74 0B 41 66 3B CF 72 DF E9 ?? ?? ?? ?? \
                    8B 5D EC 8B 7D 08";
/// The hooked entry: movzx edi,word [esi+22h] ; xor eax,eax.
const ENTRY: usize = 6;
const JAE_MISS: usize = 0x0E;
const JMP_MISS: usize = 0x3A;
const FOUND_AT: usize = 0x3F;

/// [`scan_stub`]'s exits: found, miss, and the original loop after the entry.
static FOUND: Addr = Addr::new();
static MISS: Addr = Addr::new();
static LOOP: Addr = Addr::new();

/// Replaces the scan's entry: esi = node, cx = occurrence, [ebp-14h] = name id. Exits to `found` (esi = child) or
/// `miss`, which don't read the loop's other registers.
///
/// # Safety
/// Only jumped to from that scan.
#[unsafe(naked)]
unsafe extern "C" fn scan_stub() {
    naked_asm!(
        "pushad",
        "movzx ecx, cx",
        "push ecx",
        "push dword ptr [ebp - 0x14]",
        "push esi",
        "call {lookup}",
        "add esp, 12",
        "mov dword ptr [esp + 28], eax", // popad restores it into eax
        "popad",
        "cmp eax, 1",
        "je 3f",
        "test eax, eax",
        "jz 2f",
        "mov esi, eax",
        "jmp dword ptr [{found}]",
        "2:",
        "jmp dword ptr [{miss}]",
        // Fallback: the moved entry, then the loop.
        "3:",
        "movzx edi, word ptr [esi + {count}]",
        "xor eax, eax",
        "jmp dword ptr [{back}]",
        lookup = sym lookup,
        found = sym FOUND,
        miss = sym MISS,
        back = sym LOOP,
        count = const COUNT,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(SCAN, "path child scan")?;
    let miss = core.branch_at(site + JAE_MISS)?.target;
    ensure!(core.branch_at(site + JMP_MISS)?.target == miss, "path child scan: miss exits differ");
    FOUND.set(site + FOUND_AT);
    MISS.set(miss);
    LOOP.set(site + ENTRY);
    init()?;
    core.jump(site, ENTRY, scan_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[site]))
}
