//! `locidx`, Linux: the append (cdecl: value lo, hi, &data, &capacity, &count) is hooked by [`entry`]. The turf/area
//! gatherer's four inlined unique-append scans (objs and mobs of a turf or area, and of a turf's overlap list) each
//! become a call of [`contains`] and a jump to the scan's own found or not-found target. Its vector: &data at
//! [esp+30h], &count at [esp+84h].

use super::{contains, entry, init};
use crate::base::code::{Cond, Stub, hx};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The append: esi = &capacity, ebx = &count, edi = &data ; count < capacity: store ; else grow, and on overflow
/// (`je`, its rel8 at [`OVERFLOW_JE`]) report it.
const APPEND: &str = "57 56 53 83 EC 10 8B 74 24 2C 8B 5C 24 30 F3 0F 7E 44 24 20 8B 7C 24 28 8B 13 8B 06 39 C2 72 ?? \
                      89 C2 89 C1 C1 EA 02 F7 D1 83 C2 08 39 D1 73 ?? BA F8 FF FF FF B8 FF FF FF FF 85 C9 74";
const OVERFLOW_JE: usize = 61;
/// Its first instructions, moved by the hook: push edi ; push esi ; push ebx ; sub esp,10h.
const ENTRY: usize = 6;
/// The objs (ebp:ebx = value; scan compares `38 0C C2`) and mobs (`3A 0C C2`) of a turf or area: ecx = count ;
/// none: not found ; edx = data ; loop { equal: on past the scan (found) ; end: not found }.
const OBJS: &str = "8B 84 24 84 00 00 00 8B 08 85 C9 0F 84 ?? ?? ?? ?? 8B 44 24 30 89 4C 24 44 89 E9 8B 6C 24 44 8B 10 \
                    31 C0 EB 0E 8D 76 00 83 C0 01 39 E8 0F 84 ?? ?? ?? ?? 38 0C C2 75 F0 39 5C C2 04 75 EA \
                    8D B4 26 00 00 00 00 8D 76 00";
const MOBS: &str = "8B 84 24 84 00 00 00 8B 08 85 C9 0F 84 ?? ?? ?? ?? 8B 44 24 30 89 4C 24 20 89 E9 8B 6C 24 20 8B 10 \
                    31 C0 EB 0E 8D 76 00 83 C0 01 39 E8 0F 84 ?? ?? ?? ?? 3A 0C C2 75 F0 3B 5C C2 04 75 EA \
                    8D B4 26 00 00 00 00 8D 76 00";
/// The overlap list's mobs (type 3, data esi): the scan from +12h, found = past it.
const OVER_MOBS: &str = "F3 0F 7E 00 8B 84 24 84 00 00 00 8B 08 0F 29 44 24 10 85 C9 0F 84 ?? ?? ?? ?? 8B 44 24 30 8B 10 \
                         31 C0 EB 10 8D 74 26 00 90 83 C0 01 39 C8 0F 84 ?? ?? ?? ?? 80 3C C2 03 75 EF 39 74 C2 04 75 E9 90";
/// The overlap list's objs (type 2, data esi): the scan from +12h, found = the jmp at its end.
const OVER_OBJS: &str = "F3 0F 7E 00 8B 84 24 84 00 00 00 8B 08 0F 29 44 24 10 85 C9 74 ?? 8B 44 24 30 8B 10 31 C0 EB 10 \
                         8D B4 26 00 00 00 00 66 90 83 C0 01 39 C8 74 ?? 80 3C C2 02 75 F3 39 74 C2 04 75 ED EB";
/// Each not-found target: the append (called at [`MISSING_CALL`]) of the value at `[esp]`, `[esp+4]` (or +10h, +14h)
/// to the vector.
const MISSING: &str = "83 EC 0C FF B4 24 90 00 00 00 FF B4 24 90 00 00 00 FF 74 24 44 FF 74 24 ?? FF 74 24 ?? E8";
const MISSING_CALL: usize = 29;
/// How far apart the four scans may be: they are one function.
const SPAN: usize = 0x600;

/// A scan to replace: where, how long, the push of the value's type and data, and its found and not-found targets.
struct Scan {
    at: usize,
    len: usize,
    push_type: &'static [u8],
    push_data: u8,
    found: usize,
    missing: usize,
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let text = core.find_data(b"Overflow in AddToLocList\n\0")?.unique("loc list overflow message")?;
    let add = core.find_unique(APPEND, "loc list append")?;
    core.require(
        core.rel8(add + OVERFLOW_JE)?,
        &format!("C7 44 24 20 {} 83 C4 10 5B 5E 5F E9", hx(text)),
        "loc list append: overflow message",
    )?;

    let objs = core.find_unique(OBJS, "loc list scan (objs)")?;
    let mobs = core.find_unique(MOBS, "loc list scan (mobs)")?;
    let over_mobs = core.find_unique(OVER_MOBS, "loc list scan (overlap mobs)")?;
    let over_objs = core.find_unique(OVER_OBJS, "loc list scan (overlap objs)")?;
    ensure!(
        objs < mobs && mobs < over_mobs && over_mobs < over_objs && over_objs - objs < SPAN,
        "loc list scans: not one function"
    );
    // Both not-found exits of a scan agree: (first, second) branch target.
    let one = |(a, b): (usize, usize)| -> Result<usize> {
        ensure!(a == b, "loc list scan: two not-found targets");
        Ok(a)
    };
    let rel32 = |a: usize, b: usize| -> Result<usize> { one((core.rel32(a)?, core.rel32(b)?)) };
    let over_found = over_mobs + 0x41;
    ensure!(core.rel8(over_objs + 0x3D)? == over_found, "loc list scan (overlap objs): unexpected targets");
    // A turf's or area's objs or mobs: push ebp ; push ebx ; found = past the scan.
    let own = |at: usize| -> Result<Scan> {
        let missing = rel32(at + 13, at + 0x2F)?;
        Ok(Scan { at, len: 0x48, push_type: &[0x55], push_data: 0x53, found: at + 0x48, missing })
    };
    let scans = [
        own(objs)?,
        own(mobs)?,
        Scan {
            at: over_mobs + 0x12,
            len: 0x2F,
            push_type: &[0x6A, 3],
            push_data: 0x56,
            found: over_found,
            missing: rel32(over_mobs + 0x16, over_mobs + 0x30)?,
        },
        Scan {
            at: over_objs + 0x12,
            len: 0x2C,
            push_type: &[0x6A, 2],
            push_data: 0x56,
            found: over_found,
            missing: one((core.rel8(over_objs + 0x15)?, core.rel8(over_objs + 0x2F)?))?,
        },
    ];
    ensure!(scans[3].missing == scans[2].missing, "loc list scan (overlap objs): unexpected targets");
    for s in &scans {
        ensure!(
            core.is_at(s.missing, MISSING)? && core.call_target(s.missing + MISSING_CALL)? == add,
            "loc list scan at {:#x}: not-found path doesn't append",
            s.at
        );
    }

    core.untouched(&scans.iter().map(|s| (s.at, s.len)).collect::<Vec<_>>())?;
    for s in &scans {
        let mut stub = Stub::new();
        stub.bytes(&[0xFF, 0xB4, 0x24, 0x84, 0x00, 0x00, 0x00]) // push [esp+84h] (&count)
            .bytes(&[0xFF, 0x74, 0x24, 0x34]) // push [esp+34h] (&data)
            .bytes(&[s.push_data]) // push data
            .bytes(s.push_type) // push type
            .call(contains as *const ())
            .bytes(&[0x83, 0xC4, 0x10]) // add esp,10h
            .bytes(&[0x85, 0xC0]) // test eax,eax
            .jcc(Cond::NotEqual, s.found) // jne found
            .jmp(s.missing); // jmp not found
        core.write_stub(s.at, s.len, &stub)?;
    }
    init(core.hook(add, ENTRY, entry as *const ())?)?;
    let suffix = if verify() { " (verify)" } else { "" };
    Ok(format!("{}{suffix}", core.report(&[add, objs])))
}
