//! `turfpf`, Windows: the turf loop's call of the contents lookup (cdecl, [esp+4] = turf id) calls [`stub`] instead.

use std::arch::naked_asm;

use super::{Globals, hook, prefetch};
use crate::base::code::read;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The row loop of SendMaps' turf scan, from the row width [ebp-54h] (turfs per row, bx) on: per row, [ebp-4Ch] = rows
/// left (low word, minus one), eax = turfs left (bx); per turf, [ebp-44h] = turfs left minus one; turf edi without its
/// flag (the flags global at +3Ah): next; contents lookup (+49h); each object (by id at +72h, next id +48h) and mob
/// (by id at +0D2h) checked; next turf (edi+1) while the low word of [ebp-44h] isn't 0; then edi += world.maxx
/// (+15Fh) - [ebp-54h] and the next row while rows are left. Nothing in it writes those three slots elsewhere.
const LOOP: &str = "8B 5D E4 0F B7 C3 89 45 AC 0F 1F 84 00 00 00 00 00 81 C1 FF FF 00 00 0F B7 C3 89 4D B4 66 85 DB 0F 84 43 01 \
     00 00 66 66 66 0F 1F 84 00 00 00 00 00 05 FF FF 00 00 89 45 BC A1 ?? ?? ?? ?? 80 3C 07 00 0F 84 04 01 00 00 \
     57 E8 ?? ?? ?? ?? 8B D8 83 C4 04 89 5D C0 85 DB 0F 84 EE 00 00 00 8B 43 08 89 45 C4 3D FF FF 00 00 74 56 66 \
     0F 1F 44 00 00 50 E8 ?? ?? ?? ?? 8B F8 83 C4 04 85 FF 74 3E 8B 5F 48 46 8B 4D D4 53 57 FF 75 C4 E8 ?? ?? ?? \
     ?? 09 45 FC 8B 47 50 85 C0 74 16 8D 4D F8 51 8D 4D FC 51 50 FF 75 F0 E8 ?? ?? ?? ?? 83 C4 10 03 F0 89 5D C4 \
     8B C3 81 FB FF FF 00 00 75 B3 8B 5D C0 8B 43 0C 89 45 C4 3D FF FF 00 00 74 53 0F 1F 00 50 E8 ?? ?? ?? ?? 8B \
     F8 83 C4 04 85 FF 74 3E 8B 5F 48 46 8B 4D CC 53 57 FF 75 C4 E8 ?? ?? ?? ?? 09 45 F8 8B 47 50 85 C0 74 16 8D \
     4D F8 51 8D 4D FC 51 50 FF 75 F0 E8 ?? ?? ?? ?? 83 C4 10 03 F0 89 5D C4 8B C3 81 FB FF FF 00 00 75 B3 8B 5D \
     C0 83 7B 14 00 8B 7D EC 74 22 8B 4D B8 53 57 E8 ?? ?? ?? ?? 8D 45 F8 50 8D 45 FC 50 FF 73 14 FF 75 F0 E8 ?? \
     ?? ?? ?? 83 C4 10 03 F0 8B 45 BC 47 89 7D EC 66 85 C0 0F 85 D5 FE FF FF 66 8B 15 ?? ?? ?? ?? 8B 4D B4 8B 5D \
     E4 0F BF C2 2B 45 AC 03 F8 89 7D EC 66 85 C9 0F 85 94 FE FF FF";
const FLAGS: usize = 0x3A;
const LOOKUP_CALL: usize = 0x49;
const OBJ_CALL: usize = 0x72;
const MOB_CALL: usize = 0xD2;
const MAXX: usize = 0x15F;
/// The contents lookup: id < turf count (+8), flag byte (+0Fh) set, bucket [buckets (+26h) + (mask (+1Ch, word) & id
/// low word) * 4], chain node +0 id, +4 next; else 0.
const LOOKUP: &str = "55 8B EC 8B 55 08 3B 15 ?? ?? ?? ?? 73 2E A1 ?? ?? ?? ?? 80 3C 10 00 74 23 0F B7 0D ?? ?? ?? ?? 0F B7 C2 \
     23 C8 A1 ?? ?? ?? ?? 8B 04 88 85 C0 74 0B 39 10 74 09 8B 40 04 85 C0 75 F5 33 C0 5D C3";
/// Object or mob by id: id < count (+8) ? table (+13h)\[id\] : 0.
const BY_ID: &str = "55 8B EC 8B 4D 08 3B 0D ?? ?? ?? ?? 72 04 33 C0 5D C3 A1 ?? ?? ?? ?? 8B 04 88 5D C3";

/// The stub's call: turf `t` and the loop's frame `ebp` (its three slots, see [`LOOP`]); returns the lookup.
extern "C" fn enter(t: u32, ebp: usize) -> usize {
    // SAFETY: `ebp` is the turf loop's frame (only the stub calls this).
    unsafe { prefetch(t, ebp, read::<u16>(ebp - 0x44) as u32, read(ebp - 0x54), read::<u16>(ebp - 0x4C) as u32) }
}

/// [`enter`] (turf id = the call's argument, ebp), then the lookup with the call's stack. Naked because Rust can't
/// read the caller's ebp; clobbers only eax, ecx, edx, and the flags, like the cdecl call it replaces.
///
/// # Safety
/// Only called from the turf loop's lookup call.
#[unsafe(naked)]
unsafe extern "C" fn stub() {
    naked_asm!(
        "push ebp",
        "push dword ptr [esp + 8]",
        "call {enter}",
        "add esp, 8",
        "jmp eax",
        enter = sym enter,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let l = core.find_unique(LOOP, "turf loop")?;
    let call = l + LOOKUP_CALL;
    let lookup = core.follow_call(call, LOOKUP, "turf contents lookup")?;
    ensure!(core.u32_at(lookup + 0x0F)? == core.u32_at(l + FLAGS)?, "turf contents lookup: not the loop's turf flags");
    let obj = core.follow_call(l + OBJ_CALL, BY_ID, "object by id")?;
    let mob = core.follow_call(l + MOB_CALL, BY_ID, "mob by id")?;
    let p = |at: usize| core.ptr_at(at);
    let globals = Globals {
        turfs: p(lookup + 0x08)?,
        flags: p(lookup + 0x0F)?,
        mask: p(lookup + 0x1C)?,
        buckets: p(lookup + 0x26)?,
        mobs: (p(mob + 0x08)?, p(mob + 0x13)?),
        objs: (p(obj + 0x08)?, p(obj + 0x13)?),
        maxx: p(l + MAXX)?,
        lookup,
    };
    hook(cx, call, globals, stub)
}
