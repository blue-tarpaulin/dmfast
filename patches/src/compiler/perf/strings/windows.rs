//! `strings`, Windows: the scan jumps to [`scan_stub`], the save's free store to [`free_stub`].

use std::arch::naked_asm;

use super::{HINT, verify};
use crate::base::code::{Addr, hx};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::string_table::free_store;

/// The insert's scan: mov edi,[count] ; mov ecx,[table] ; xor esi,esi ; test edi,edi ; je grow ;
/// loop: cmp [ecx+esi*4],0 ; jne next ; cmp esi,0FFFFh ; jne found ; next: inc esi ; cmp esi,edi ; jb loop ; jmp grow
const SCAN: &str = "8B 3D ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 33 F6 85 FF 74 ?? \
                    83 3C B1 00 75 08 81 FE FF FF 00 00 75 ?? 46 3B F7 72 ED EB ??";
/// [`scan_stub`] replaces the scan from `xor esi,esi` to its end.
const SCAN_FROM: usize = 12;
const SCAN_END: usize = 39;
/// The rel8 fields of `je grow`, `jne found`, and `jmp grow`.
const JE_GROW: usize = 17;
const JNE_FOUND: usize = 31;
const JMP_GROW: usize = 38;
/// The free store's length.
const FREE_LEN: usize = 7;

/// `mov [count],eax` in the grow code, which continues with the new count in eax.
static GROW_STORE: Addr = Addr::new();
/// `cmp esi,edi ; jb use`: the free slot esi is used.
static FOUND: Addr = Addr::new();
static FREE_RET: Addr = Addr::new();

/// Replaces the scan from `xor esi,esi` on. edi = count, ecx = table.
///
/// # Safety
/// Only jumped to from the scan.
#[unsafe(naked)]
unsafe extern "C" fn scan_stub() {
    naked_asm!(
        "mov esi, dword ptr [{hint}]",
        "cmp esi, edi",
        "jbe 2f",
        "xor esi, esi",
        "2:",
        "cmp esi, edi",
        "jae 4f",
        "3:",
        "cmp dword ptr [ecx + esi*4], 0",
        "jne 5f",
        "cmp esi, 0xffff",
        "jne 4f",
        "5:",
        "inc esi",
        "cmp esi, edi",
        "jb 3b",
        "4:",
        "lea eax, [esi + 1]",
        "mov dword ptr [{hint}], eax",
        "cmp esi, edi",
        "jb 6f",
        // grow: new count = max(count*2, needed+10), continue at `mov [count],eax`
        "lea eax, [edi + edi]",
        "lea edx, [esi + 10]",
        "cmp eax, edx",
        "jae 7f",
        "mov eax, edx",
        "7:",
        "jmp dword ptr [{grow}]",
        "6:",
        "jmp dword ptr [{found}]",
        hint = sym HINT,
        grow = sym GROW_STORE,
        found = sym FOUND,
    )
}

/// [`scan_stub`] after [`verify::check`] (`DMFAST_VERIFY`).
///
/// # Safety
/// Only jumped to from the scan.
#[unsafe(naked)]
unsafe extern "C" fn verify_stub() {
    naked_asm!(
        "pushad",
        "mov ebp, esp",
        "and esp, -16",
        "sub esp, 4",
        "push dword ptr [{hint}]",
        "push edi",
        "push ecx",
        "call {check}",
        "mov esp, ebp",
        "popad",
        "jmp {scan}",
        hint = sym HINT,
        check = sym verify::check,
        scan = sym scan_stub,
    )
}

/// Replaces the free store: free slot ecx, lower the hint to it.
///
/// # Safety
/// Only jumped to from the free store.
#[unsafe(naked)]
unsafe extern "C" fn free_stub() {
    naked_asm!(
        "mov dword ptr [eax + ecx*4], 0",
        "cmp ecx, dword ptr [{hint}]",
        "jae 2f",
        "mov dword ptr [{hint}], ecx",
        "2:",
        "jmp dword ptr [{ret}]",
        hint = sym HINT,
        ret = sym FREE_RET,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let free = free_store(cx)?;
    let m = core.find_unique(SCAN, "string-table scan")?;
    let count = core.ptr_at(m + 2)?;
    ensure!(core.ptr_at(m + 8)? == free.table, "string-table scan: not the saved table");
    // found: cmp esi,edi ; jb use ; grow: lea eax,[esi+0Ah] ; mov [count],eax
    let found = core.rel8(m + JNE_FOUND)?;
    core.require(found, &format!("3B F7 72 ?? 8D 46 0A A3 {}", hx(count)), "string-table grow")?;
    let grow = found + 4;
    ensure!(core.rel8(m + JE_GROW)? == grow && core.rel8(m + JMP_GROW)? == grow, "string-table scan: grows elsewhere");
    let scan = m + SCAN_FROM;
    core.untouched(&[(scan, SCAN_END - SCAN_FROM)])?;
    FOUND.set(found);
    GROW_STORE.set(grow + 3);
    FREE_RET.set(free.at + FREE_LEN);
    let stub = if crate::base::driver::verify() { verify_stub } else { scan_stub };
    core.jump(scan, SCAN_END - SCAN_FROM, stub as unsafe extern "C" fn())?;
    core.jump(free.at, FREE_LEN, free_stub as unsafe extern "C" fn())?;
    Ok(format!("hinted scan {}", core.report(&[scan])))
}
