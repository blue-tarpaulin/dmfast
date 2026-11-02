//! `locidx`, Windows: the append (cdecl: value lo, hi, &data, &capacity, &count) is hooked by [`entry`]. The unique
//! append (same arguments) skips null, `""`, 0, and null-object values, scans the vector, and appends on a miss; the
//! scan becomes a call of [`contains`].

use super::{contains, entry, init};
use crate::base::code::{Cond, Stub, hx};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The append: ebx = &data, esi = &capacity, edi = &count ; count < capacity: store ; else grow, and on overflow
/// report it (the message's address follows).
const APPEND: &str = "55 8B EC 53 8B 5D 10 56 8B 75 14 57 8B 7D 18 8B 0E 39 0F 72 ?? 8B D1 8B C1 C1 EA 02 F7 D0 83 C2 08 \
                      3B C2 73 ?? 85 C0 75 ?? 68";
/// Its first instructions, moved by the hook: push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+10h].
const ENTRY: usize = 7;
/// The unique append. Its scan, from [`SCAN`] to `SCAN + SCAN_LEN`: edi = count ; none: on to the append ;
/// edx = data ; loop { [edx+eax*8] == cl and [edx+eax*8+4] == esi: to [`FOUND`] (return) } ; then the append
/// (push ebx (&count), [ebp+14h], [ebp+10h] (&data), esi, ecx ; call) at [`CALL`].
const UNIQUE: &str = "55 8B EC 8B 4D 08 84 C9 74 ?? 80 F9 22 74 ?? 56 8B 75 0C 80 F9 2A 75 ?? F3 0F 10 45 0C 0F 2E 05 \
                      ?? ?? ?? ?? 9F F6 C4 44 7B ?? EB ?? 80 F9 06 75 ?? 85 F6 74 ?? 53 8B 5D 18 33 C0 57 8B 3B 85 FF 74 15 \
                      8B 55 10 8B 12 38 0C C2 75 06 39 74 C2 04 74 16 40 3B C7 72 F0 53 FF 75 14 FF 75 10 56 51 E8 ?? ?? ?? ?? \
                      83 C4 14 5F 5B 5E 5D C3";
const SCAN: usize = 0x3C;
const SCAN_LEN: usize = 0x1B;
const FOUND: usize = 0x68;
const CALL: usize = 0x60;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let text = core.find_data(b"Overflow in AddToLocList\n\0")?.unique("loc list overflow message")?;
    let add = core.find_unique(&format!("{APPEND} {} E8", hx(text)), "loc list append")?;
    let unique = core.find_unique(UNIQUE, "loc list unique append")?;
    ensure!(core.call_target(unique + CALL)? == add, "loc list unique append: doesn't call the append");

    // The stock scan leaves ebx, esi, and ecx as the append wants them.
    let at = unique + SCAN;
    let mut s = Stub::new();
    s.bytes(&[0x53]) // push ebx
        .bytes(&[0xFF, 0x75, 0x10]) // push [ebp+10h]
        .bytes(&[0x56]) // push esi
        .bytes(&[0x51]) // push ecx
        .call(contains as *const ())
        .bytes(&[0x83, 0xC4, 0x10]) // add esp,10h
        .bytes(&[0x8B, 0x4D, 0x08]) // mov ecx,[ebp+8]
        .bytes(&[0x85, 0xC0]) // test eax,eax
        .jcc8(Cond::NotEqual, unique + FOUND); // jne FOUND ; NOPs: on to the append
    core.untouched(&[(at, SCAN_LEN)])?;
    core.write_stub(at, SCAN_LEN, &s)?;
    init(core.hook(add, ENTRY, entry as *const ())?)?;
    let suffix = if verify() { " (verify)" } else { "" };
    Ok(format!("{}{suffix}", core.report(&[add, unique])))
}
