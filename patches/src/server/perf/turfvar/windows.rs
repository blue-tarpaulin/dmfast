//! `turfvar`, Windows.

use crate::base::Result;
use crate::base::feature::Ctx;

/// Cdecl turf ref; esi = hash, ecx = buckets, edx = index.
/// Stock: `eax = buckets[edx]; edi = &buckets[edx]; while (eax && [eax] != esi) eax = [eax+4]`.
/// New: `edi = &buckets[edx]; eax = *edi; while (eax && [eax] != esi) { edi = eax+4; eax = *edi }`, so a miss
/// inserts at `*edi`, the tail.
pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    const FIND: &str = "55 8B EC 56 57 FF 75 0C FF 75 08 E8 ?? ?? ?? ?? 0F B7 15 ?? ?? ?? ?? 8B F0 0F B7 CE 83 C4 08 \
                        23 D1 8B 0D ?? ?? ?? ?? 8B 04 91 8D 3C 91 85 C0 74 0B 39 30 74 55 8B 40 04 85 C0 75 F5 \
                        6A 18 FF 15 ?? ?? ?? ?? 83 C4 04 85 C0 75 ?? E8 ?? ?? ?? ?? 5F 33 C0 5E 5D C3 33 C9 89 30 \
                        89 48 0C 66 8B 0D ?? ?? ?? ?? 66 89 48 10 66 41 66 89 0D ?? ?? ?? ?? 8B 0F C7 40 08 00 00 00 00 \
                        C7 40 14 00 00 00 00 C6 40 12 00 89 48 04 89 07 5F 5E 5D C3";
    #[rustfmt::skip]
    const NEW: [u8; 21] = [
        0x8D, 0x3C, 0x91,    //        lea edi,[ecx+edx*4]
        0x8B, 0x07,          // back:  mov eax,[edi]
        0x85, 0xC0,          //        test eax,eax
        0x74, 0x0C,          //        jz insert        (stock insert code, edi = null tail link)
        0x39, 0x30,          //        cmp [eax],esi
        0x74, 0x56,          //        je found         (stock return)
        0x8D, 0x78, 0x04,    //        lea edi,[eax+4]
        0xEB, 0xF1,          //        jmp back
        0x0F, 0x1F, 0x00,    //        nop (3 bytes, pads to the stock loop's length)
    ];
    let at = core.find_unique(FIND, "turf var find-or-create")? + 0x27;
    core.untouched(&[(at, NEW.len())])?;
    core.write_code(at, &NEW)?;
    Ok(format!("at {:#x}", at - core.base))
}
