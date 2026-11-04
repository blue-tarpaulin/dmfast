//! `strings`, Linux: the scan jumps to [`scan_stub`], the `count == 0` branch to [`empty_stub`], and the save's free
//! store to [`free_stub`].

use std::arch::naked_asm;

use super::{HINT, verify};
use crate::base::Result;
use crate::base::code::{Addr, hx};
use crate::base::feature::Ctx;

/// The insert's scan: mov ebx,[count] ; mov edx,[table] ; test ebx,ebx ; je empty ; mov eax,[edx] ; test eax,eax ;
/// je slot0 ; lea eax,[edx+4] ; xor edi,edi ; loop: mov ecx,edi ; add edi,1 ; cmp ebx,edi ; jbe full ;
/// mov ecx,eax ; add eax,4 ; cmp [ecx],0 ; jne loop ; cmp edi,0FFFFh ; je loop ; found: ...
const SCAN: &str = "8B 1D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 85 DB 0F 84 ?? ?? ?? ?? \
                    8B 02 85 C0 0F 84 ?? ?? ?? ?? 8D 42 04 31 FF \
                    89 F9 83 C7 01 39 FB 0F 86 ?? ?? ?? ?? \
                    89 C1 83 C0 04 83 39 00 75 E9 81 FF FF FF 00 00 74 E1";
/// The rel32 fields of `je empty`, `je slot0`, and `jbe full`.
const JE_EMPTY: usize = 16;
const JE_SLOT0: usize = 26;
const JBE_FULL: usize = 44;
/// [`scan_stub`] replaces the scan from the slot-0 check to the found label.
const SCAN_FROM: usize = 20;
const SCAN_END: usize = 66;
/// The free store's length.
const FREE_LEN: usize = 6;

/// The global holding the table's address.
static TABLE_VAR: Addr = Addr::new();
static EMPTY: Addr = Addr::new();
static SLOT0: Addr = Addr::new();
static FOUND: Addr = Addr::new();
static GROW: Addr = Addr::new();
static FREE_RET: Addr = Addr::new();

/// Replaces the scan after the `count != 0` check. ebx = count, edx = table; ebp and esi are live.
/// Exits like the original: slot 0 free -> `slot0` (edx = table); slot edi free -> `found` with
/// ecx = `&table[edi]`; table full -> `grow` with edi = count, ecx = new count, eax = new size in bytes.
///
/// # Safety
/// Only jumped to from the scan.
#[unsafe(naked)]
unsafe extern "C" fn scan_stub() {
    naked_asm!(
        "mov edi, dword ptr [{hint}]",
        "cmp edi, ebx",
        "jbe 3f",
        "xor edi, edi",
        "3:",
        "test edi, edi",
        "jnz 4f",
        "cmp dword ptr [edx], 0",
        "jne 31f",
        "mov dword ptr [{hint}], 1",
        "jmp dword ptr [{slot0}]",
        "31:",
        "mov edi, 1",
        "4:",
        "cmp edi, ebx",
        "jae 7f",
        "5:",
        "cmp dword ptr [edx + edi*4], 0",
        "jne 6f",
        "cmp edi, 0xffff",
        "jne 8f",
        "6:",
        "inc edi",
        "cmp edi, ebx",
        "jb 5b",
        // full (edi = count): new count = max(count*2, count+10)
        "7:",
        "lea eax, [edi + 1]",
        "mov dword ptr [{hint}], eax",
        "lea ecx, [ebx + ebx]",
        "lea eax, [ebx + 10]",
        "cmp ecx, eax",
        "jae 71f",
        "mov ecx, eax",
        "71:",
        "lea eax, [ecx*4]",
        "jmp dword ptr [{grow}]",
        "8:",
        "lea eax, [edi + 1]",
        "mov dword ptr [{hint}], eax",
        "lea ecx, [edx + edi*4]",
        "jmp dword ptr [{found}]",
        hint = sym HINT,
        slot0 = sym SLOT0,
        grow = sym GROW,
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
        "push ebx",
        "push edx",
        "call {check}",
        "mov esp, ebp",
        "popad",
        "jmp {scan}",
        hint = sym HINT,
        check = sym verify::check,
        scan = sym scan_stub,
    )
}

/// Target of the `count == 0` branch, where the insert creates the table: no slot is in use.
///
/// # Safety
/// Only jumped to from the `count == 0` branch.
#[unsafe(naked)]
unsafe extern "C" fn empty_stub() {
    naked_asm!(
        "mov dword ptr [{hint}], 0",
        "jmp dword ptr [{empty}]",
        hint = sym HINT,
        empty = sym EMPTY,
    )
}

/// Replaces the free store (eax = `&table[i]`): free the slot, lower the hint to i.
///
/// # Safety
/// Only jumped to from the free store.
#[unsafe(naked)]
unsafe extern "C" fn free_stub() {
    naked_asm!(
        "mov dword ptr [eax], 0",
        "push eax",
        "push ecx",
        "mov ecx, dword ptr [{table_var}]",
        "sub eax, dword ptr [ecx]",
        "shr eax, 2",
        "cmp eax, dword ptr [{hint}]",
        "jae 2f",
        "mov dword ptr [{hint}], eax",
        "2:",
        "pop ecx",
        "pop eax",
        "jmp dword ptr [{ret}]",
        table_var = sym TABLE_VAR,
        hint = sym HINT,
        ret = sym FREE_RET,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let m = core.find_unique(SCAN, "string-table scan")?;
    let (count_var, table_var) = (core.ptr_at(m + 2)?, core.ptr_at(m + 8)?);
    // full: add ecx,0Bh ; lea eax,[ecx*4] ; jmp grow ; grow: sub esp,8 ; mov [count],ecx
    let full = core.rel32(m + JBE_FULL)?;
    core.require(full, "83 C1 0B 8D 04 8D 00 00 00 00 E9", "string-table full path")?;
    let grow = core.rel32(full + 11)?;
    core.require(grow, &format!("83 EC 08 89 0D {}", hx(count_var)), "string-table grow")?;
    // The save's free store, the only store of a null: add eax,[table] ; sub esp,8 ; mov dword [eax],0 ; push 0 ;
    // push ebx ; call
    let store = core.find_unique(
        &format!("03 05 {} 83 EC 08 C7 00 00 00 00 00 6A 00 53 E8", hx(table_var)),
        "string-table free store",
    )? + 9;
    let scan = m + SCAN_FROM;
    core.untouched(&[(scan, SCAN_END - SCAN_FROM)])?;
    TABLE_VAR.set(table_var);
    EMPTY.set(core.rel32(m + JE_EMPTY)?);
    SLOT0.set(core.rel32(m + JE_SLOT0)?);
    FOUND.set(m + SCAN_END);
    GROW.set(grow);
    FREE_RET.set(store + FREE_LEN);
    core.retarget(m + JE_EMPTY, empty_stub as unsafe extern "C" fn())?;
    let stub = if crate::base::driver::verify() { verify_stub } else { scan_stub };
    core.jump(scan, SCAN_END - SCAN_FROM, stub as unsafe extern "C" fn())?;
    core.jump(store, FREE_LEN, free_stub as unsafe extern "C" fn())?;
    Ok(format!("hinted scan {}", core.report(&[scan])))
}
