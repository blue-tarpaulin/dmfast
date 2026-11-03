//! `children`, Linux: the scan jumps to [`scan_stub`] at `test dx,dx ; je miss`: eax = node, dx = count, ebx = name
//! id, the occurrence in the u16 at [ebp-30h]. Exits to `found` with edx = child, or to `miss`; neither reads the
//! loop's other registers.

use std::arch::naked_asm;

use super::{init, lookup};
use crate::base::code::Addr;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// test dx,dx ; je miss ; mov eax,[eax+18h] ; sub edx,1 ; movzx ecx,word [ebp-30h] ; movzx edx,dx ;
/// lea esi,[eax+edx*4+4] ; jmp first ; next: add eax,4 ; cmp esi,eax ; je miss ;
/// first: mov edx,[eax] ; cmp ebx,[edx] ; jne next ; lea edi,[ecx-1] ; test cx,cx ; je found ; mov ecx,edi ; jmp next
const SCAN: &str = "66 85 D2 0F 84 ?? ?? ?? ?? 8B 40 18 83 EA 01 0F B7 4D D0 0F B7 D2 8D 74 90 04 EB ?? \
                    8D 74 26 00 83 C0 04 39 C6 0F 84 ?? ?? ?? ?? 8B 10 3B 1A 75 ?? 8D 79 FF 66 85 C9 \
                    0F 84 ?? ?? ?? ?? 89 F9 EB";
/// The hooked entry check: test dx,dx ; je miss.
const ENTRY: usize = 9;
const JE_MISS: usize = 3;
const LOOP_MISS: usize = 37;
const JE_FOUND: usize = 55;
/// The scan is entered from movzx edx,word [eax+1Eh] ; je scan: dx is the count, which [`lookup`] reads itself. A
/// common sequence: only its jump into the scan is relied on.
const CALLER: &str = "0F B7 50 1E 0F 84 ?? ?? ?? ??";
const CALLER_JUMP: usize = 4;

/// [`scan_stub`]'s exits: found, miss, and the scan after its entry check (`mov eax,[eax+18h]`).
static FOUND: Addr = Addr::new();
static MISS: Addr = Addr::new();
static LOOP: Addr = Addr::new();

/// # Safety
/// Only jumped to from the scan.
#[unsafe(naked)]
unsafe extern "C" fn scan_stub() {
    naked_asm!(
        "pushad",
        "mov esi, esp",
        "and esp, -16",
        "sub esp, 4",
        "movzx ecx, word ptr [ebp - 0x30]",
        "push ecx",
        "push ebx",
        "push eax",
        "call {lookup}",
        "mov esp, esi",
        "mov dword ptr [esp + 24], eax", // popad restores it into ecx (dead at the scan entry)
        "popad",
        "cmp ecx, 1",
        "je 3f",
        "test ecx, ecx",
        "jz 2f",
        "mov edx, ecx",
        "jmp dword ptr [{found}]",
        "2:",
        "jmp dword ptr [{miss}]",
        // Fallback: the original entry check, then its loop.
        "3:",
        "test dx, dx",
        "je 2b",
        "jmp dword ptr [{back}]",
        lookup = sym lookup,
        found = sym FOUND,
        miss = sym MISS,
        back = sym LOOP,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(SCAN, "path child scan")?;
    let miss = core.branch_at(site + JE_MISS)?.target;
    ensure!(core.branch_at(site + LOOP_MISS)?.target == miss, "path child scan: miss exits differ");
    core.probe_first(CALLER, "path child scan caller", |e| {
        core.branch_at(e + CALLER_JUMP).is_ok_and(|b| b.target == site)
    })?;
    FOUND.set(core.branch_at(site + JE_FOUND)?.target);
    MISS.set(miss);
    LOOP.set(site + ENTRY);
    init()?;
    core.jump(site, ENTRY, scan_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[site]))
}
