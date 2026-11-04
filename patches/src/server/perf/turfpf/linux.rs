//! `turfpf`, Linux: the turf loop's call of the contents lookup (cdecl, [esp+4] = turf id, also in ebx) calls
//! [`stub`] instead.

use std::arch::naked_asm;

use super::{Globals, hook, prefetch};
use crate::base::code::{Cond, read};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The turf loop of SendMaps' scan, from its next turf (ebx+1, until the row end [ebp-488h]) on: turf ebx without its
/// flag (the flags global at +10h): next; contents lookup (+1Eh); object list (+8, out of line: `jne` at +38h) and mob
/// list (+0Ch, `jne` at +4Ch); per turf the rest; then ebx = world.maxx (+0A1h) - width [ebp-4C8h] + row end, and
/// back to the row head (its last instruction, a `jne`) while rows ([ebp-4C4h]) are left.
const TURF: &str = "83 C3 01 3B 9D 78 FB FF FF 0F 84 8F 00 00 00 A1 ?? ?? ?? ?? 80 3C 18 00 74 E6 83 EC 0C 53 E8 ?? ?? ?? \
     ?? 83 C4 10 89 85 ?? FB FF FF 85 C0 74 D0 8B 40 08 3D FF FF 00 00 0F 85 ?? ?? ?? ?? 8B 85 ?? FB FF FF 8B 40 0C 3D \
     FF FF 00 00 0F 85 ?? ?? ?? ?? 8B B5 ?? FB FF FF 8B 46 14 85 C0 74 A1 83 EC 04 56 53 83 C3 01 FF B5 ?? FB FF FF E8 \
     ?? ?? ?? ?? 59 8B 56 14 FF B5 7C FB FF FF 8B 85 ?? FB FF FF 8D 8D B0 FB FF FF E8 ?? ?? ?? ?? 83 C4 10 01 C7 3B 9D \
     78 FB FF FF 0F 85 71 FF FF FF 0F BF 05 ?? ?? ?? ?? 66 83 AD 3C FB FF FF 01 0F B7 B5 3C FB FF FF 89 C3 2B 9D 38 FB \
     FF FF 03 9D 78 FB FF FF 66 83 FE FF 0F 85 ?? ?? ?? ??";
const FLAGS: usize = 0x10;
const LOOKUP_CALL: usize = 0x1E;
const OBJ_JNE: usize = 0x38;
const MOB_JNE: usize = 0x4C;
const MAXX: usize = 0xA1;
/// The `jne` back to the row head: [`TURF`]'s last 6 bytes.
const ROW_JNE: usize = 0xC6;
/// The row head, right before [`TURF`] (one form per compiler output): row end [ebp-488h] = width [ebp-4C8h] + ebx;
/// no turfs: out (jmp); else the first turf.
const HEADS: [&str; 3] = [
    "8B B5 38 FB FF FF 01 DE 66 83 BD 10 FB FF FF 00 89 B5 78 FB FF FF 75 1E E9 ?? ?? ?? ?? 8D B4 26 00 00 00 00 8D 76 00",
    "8B B5 38 FB FF FF 66 83 BD 12 FB FF FF 00 8D 0C 1E 89 8D 78 FB FF FF 75 1E E9 ?? ?? ?? ?? 8D B4 26 00 00 00 00 8D 76 00",
    "8B 8D 38 FB FF FF 01 D9 66 83 BD 10 FB FF FF 00 89 8D 78 FB FF FF 75 17 E9 ?? ?? ?? ?? 8D 76 00",
];
/// The contents lookup: id < turf count (+6), flag byte (+0Dh) set, bucket [buckets (+20h) + (mask (+1Ah, word) &
/// id) * 4], chain node +0 id, +4 next; else 0.
const LOOKUP: &str = "8B 54 24 04 39 15 ?? ?? ?? ?? 76 ?? A1 ?? ?? ?? ?? 80 3C 10 00 74 ?? 0F B7 05 ?? ?? ?? ?? 8B 0D ?? ?? \
     ?? ?? 21 D0 0F B7 C0 8B 04 81 85 C0 75 0F EB 11 8D B6 00 00 00 00 8B 40 04 85 C0 74 11 3B 10 75 F5 C3";
/// An out-of-line id list walk: ebx saved, the first id into [ebp-498h], then a `jmp` (+0Ch) to its by-id call.
const LIST: &str = "89 9D ?? FB FF FF 89 85 68 FB FF FF EB";
const LIST_JMP: usize = 0x0C;
/// That call (+9).
const LIST_CALL: &str = "83 EC 0C FF B5 68 FB FF FF E8";
const BY_ID_CALL: usize = 0x09;
/// Object or mob by id: id < count (+6) ? table (+0Eh) `[id]` : 0.
const BY_ID: &str = "8B 44 24 04 39 05 ?? ?? ?? ?? 76 ?? 8B 15 ?? ?? ?? ?? 8B 04 82 C3";

/// The stub's call: turf `t` (ebx), the loop's frame `ebp` (turfs left = row end [ebp-488h] - t - 1, width
/// [ebp-4C8h], rows left [ebp-4C4h] low word); returns the lookup.
extern "C" fn enter(t: u32, ebp: usize) -> usize {
    // SAFETY: `ebp` is the turf loop's frame (only the stub calls this).
    unsafe {
        let rem = read::<u32>(ebp - 0x488).wrapping_sub(t).wrapping_sub(1);
        prefetch(t, ebp, rem, read(ebp - 0x4C8), read::<u16>(ebp - 0x4C4) as u32)
    }
}

/// [`enter`] (ebx, ebp) on a 16-byte aligned stack (esi keeps the entry esp), then the lookup with the call's stack.
/// Naked to read the caller's ebp; clobbers only eax, ecx, edx, and the flags, like the cdecl call it replaces.
///
/// # Safety
/// Only called from the turf loop's lookup call.
#[unsafe(naked)]
unsafe extern "C" fn stub() {
    naked_asm!(
        "push esi",
        "mov esi, esp",
        "and esp, -16",
        "sub esp, 8",
        "push ebp",
        "push ebx",
        "call {enter}",
        "mov esp, esi",
        "pop esi",
        "jmp eax",
        enter = sym enter,
    )
}

/// The table (count, pointer array) of the object or mob list walk the `jne` at `jne` leads to.
fn list_table(core: &Image, jne: usize, what: &str) -> Result<(usize, usize)> {
    let list = core.jcc_at(jne, Cond::NotEqual)?.target;
    core.require(list, LIST, what)?;
    let call = core.branch_at(list + LIST_JMP)?.target;
    core.require(call, LIST_CALL, what)?;
    let by_id = core.follow_call(call + BY_ID_CALL, BY_ID, what)?;
    Ok((core.ptr_at(by_id + 0x06)?, core.ptr_at(by_id + 0x0E)?))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let l = core.find_unique(TURF, "turf loop")?;
    let head = core.jcc_at(l + ROW_JNE, Cond::NotEqual)?.target;
    ensure!(
        HEADS.iter().any(|h| head + h.split_whitespace().count() == l && core.is_at(head, h).unwrap_or(false)),
        "turf loop: unexpected row head"
    );
    let call = l + LOOKUP_CALL;
    let lookup = core.follow_call(call, LOOKUP, "turf contents lookup")?;
    ensure!(core.u32_at(lookup + 0x0D)? == core.u32_at(l + FLAGS)?, "turf contents lookup: not the loop's turf flags");
    let p = |at: usize| core.ptr_at(at);
    let globals = Globals {
        turfs: p(lookup + 0x06)?,
        flags: p(lookup + 0x0D)?,
        mask: p(lookup + 0x1A)?,
        buckets: p(lookup + 0x20)?,
        mobs: list_table(core, l + MOB_JNE, "turf loop mob list")?,
        objs: list_table(core, l + OBJ_JNE, "turf loop object list")?,
        maxx: p(l + MAXX)?,
        lookup,
    };
    hook(cx, call, globals, stub)
}
