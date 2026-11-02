//! `typepath`, Windows: the check's upper id (`goto`) becomes the id of `.index`.

use super::last_operator;
use crate::base::Result;
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // mov ecx,[esi] ; cmp ecx,0DCh ; jb in ; cmp ecx,0E5h ; jbe reject ;
    // in: lea eax,[ecx-0ADh] ; cmp eax,2Eh ; ja ok
    let at = core.find_unique(
        "8B 0E 81 F9 DC 00 00 00 72 08 81 F9 E5 00 00 00 76 0B 8D 81 53 FF FF FF 83 F8 2E 77",
        "path element id check",
    )?;
    let goto = core.ptr_at(at + 12)?;
    let plus = (core.u32_at(at + 20)? as i32).wrapping_neg() as usize;
    let last = last_operator(core, plus, goto)?;
    core.write_code(at + 12, &(last as u32).to_le_bytes())?;
    Ok(core.report(&[at]))
}
