//! `turfvar`, Linux.

use crate::base::Result;
use crate::base::feature::Ctx;

/// Regparm (eax, edx = turf ref); ebx = hash, esi = &bucket.
/// Stock: `eax = *esi; while (eax && [eax] != ebx) eax = [eax+4]`.
/// New: `eax = *esi; while (eax && [eax] != ebx) { esi = eax+4; eax = *esi }`, so a miss inserts at `*esi`, the tail.
pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    const FIND: &str = "56 53 83 EC 1C 52 50 E8 ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 83 C4 10 89 C3 66 23 05 ?? ?? ?? ?? \
                        0F B7 C0 8D 34 82 8B 06 85 C0 75 0D EB 1C 8D 74 26 00 8B 40 04 85 C0 74 11 3B 18 75 F5 \
                        83 C4 14 5B 5E C3 8D B4 26 00 00 00 00 83 EC 0C 6A 18 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? \
                        0F B7 15 ?? ?? ?? ?? 89 18 C7 40 08 00 00 00 00 C7 40 0C 00 00 00 00 8D 4A 01 66 89 50 10 \
                        C7 40 14 00 00 00 00 C6 40 12 00 8B 16 66 89 0D ?? ?? ?? ?? 89 50 04 89 06 83 C4 14 5B 5E C3";
    #[rustfmt::skip]
    const NEW: [u8; 23] = [
        0x8B, 0x06,                         // back:  mov eax,[esi]
        0x85, 0xC0,                         //        test eax,eax
        0x74, 0x1E,                         //        jz insert        (stock insert code, esi = null tail link)
        0x3B, 0x18,                         //        cmp ebx,[eax]
        0x74, 0x0D,                         //        je found         (stock return)
        0x8D, 0x70, 0x04,                   //        lea esi,[eax+4]
        0xEB, 0xF1,                         //        jmp back
        0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00, // nop (8 bytes, pads to the stock loop's length)
    ];
    let at = core.find_unique(FIND, "turf var find-or-create")? + 0x24;
    core.untouched(&[(at, NEW.len())])?;
    core.write_code(at, &NEW)?;
    Ok(format!("at +{:#x}", at - core.base))
}
