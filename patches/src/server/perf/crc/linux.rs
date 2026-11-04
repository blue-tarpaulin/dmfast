//! `crc`, Linux: libbyond's CRC hooked by [`entry`]; its table fill is inlined, so [`fill_table`] enters the original
//! with nothing to checksum.

use std::arch::naked_asm;

use super::{CrcFn, FOUND, Found, entry};
use crate::base::Result;
use crate::base::code::{Addr, Cond, Foreign, hx};
use crate::base::feature::Ctx;

/// The CRC: cmp byte [ready],0 ; push esi ; push ebx ; mov ecx,[esp+14h] ; jne loop ; mov byte [ready],1 ;
/// mov eax,table ; (SSE table fill, then the loop)
const CRC: &str = "80 3D ?? ?? ?? ?? 00 56 53 8B 4C 24 14 0F 85 ?? ?? ?? ?? C6 05 ?? ?? ?? ?? 01 B8 ?? ?? ?? ??";
const READY: usize = 2;
const JNE_LOOP: usize = 0xD;
/// Past the instructions [`original`] repeats: the table fill.
const FILL: usize = 0x13;
const TABLE: usize = 0x1B;
/// The loop's start: mov edx,[esp+10h] ; mov eax,[esp+0Ch] ; lea esi,[edx+ecx]
const LOOP: &str = "8B 54 24 10 8B 44 24 0C 8D 34 0A";
/// Its step, within [`LOOP_LEN`] bytes, before the table's address: movzx ecx,byte [edx] ; mov ebx,eax ; add edx,1 ;
/// shl eax,8 ; shr ebx,18h ; xor ecx,ebx ; xor eax,[ecx*4+table]
const STEP: &str = "0F B6 0A 89 C3 83 C2 01 C1 E0 08 C1 EB 18 31 D9 33 04 8D";
const LOOP_LEN: usize = 0x30;

/// Where [`original`] goes: BYOND's flag byte's address, the table fill, and the loop.
static READY_AT: Addr = Addr::new();
static FILL_AT: Addr = Addr::new();
static LOOP_AT: Addr = Addr::new();

/// The original: its first instructions, then the rest.
#[unsafe(naked)]
unsafe extern "C" fn original(_seed: u32, _data: *const u8, _len: u32) -> u32 {
    naked_asm!(
        // cmp byte [ready],0 ; push esi ; push ebx ; mov ecx,[esp+14h] ; jne loop
        "mov eax, dword ptr [{ready}]",
        "cmp byte ptr [eax], 0",
        "push esi",
        "push ebx",
        "mov ecx, dword ptr [esp + 0x14]",
        "jne 2f",
        "jmp dword ptr [{fill}]",
        "2:",
        "jmp dword ptr [{crc_loop}]",
        ready = sym READY_AT,
        fill = sym FILL_AT,
        crc_loop = sym LOOP_AT,
    )
}

/// Fills libbyond's table: the original with nothing to checksum.
extern "C" fn fill_table() {
    // SAFETY: no data to read; `original`'s targets are set before the hook that calls this is written.
    unsafe { original(0, std::ptr::null(), 0) };
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(CRC, "CRC-32")?;
    let (ready, table) = (core.ptr_at(f + READY)?, core.ptr_at(f + TABLE)?);
    let crc_loop = core.jcc_at(f + JNE_LOOP, Cond::NotEqual)?.target;
    core.require(crc_loop, LOOP, "CRC-32 loop")?;
    core.find_unique_in(crc_loop, LOOP_LEN, &format!("{STEP} {}", hx(table)), "CRC-32 loop step")?;
    READY_AT.set(ready);
    FILL_AT.set(f + FILL);
    LOOP_AT.set(crc_loop);
    FOUND.commit(
        cx,
        Found {
            table,
            ready,
            init: Foreign::at(fill_table as extern "C" fn()),
            original: Foreign::at(original as CrcFn),
        },
    );
    core.jump(f, 6, entry as CrcFn)?;
    Ok(core.report(&[f]))
}
