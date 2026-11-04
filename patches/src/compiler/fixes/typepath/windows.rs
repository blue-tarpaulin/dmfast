//! `typepath`, Windows: the check's upper id (`goto`) becomes the id of `.index`.

use super::last_operator;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // mov ecx,[esi] ; cmp ecx,first ; jb in ; cmp ecx,goto ; jbe reject ; in: lea eax,[ecx-plus] ; cmp eax,n ; ja ok:
    // rejects plus..=plus+n and first..=goto, one range (516: 0ADh..=0DBh and 0DCh..=0E5h).
    let at = core.find_unique(
        "8B 0E 81 F9 ?? ?? 00 00 72 08 81 F9 ?? ?? 00 00 76 0B 8D 81 ?? ?? FF FF 83 F8 ?? 77",
        "path element id check",
    )?;
    let goto = core.ptr_at(at + 12)?;
    let plus = (core.u32_at(at + 20)? as i32).wrapping_neg() as usize;
    ensure!(
        core.ptr_at(at + 4)? == plus + usize::from(core.u8_at(at + 26)?) + 1,
        "path element id check: not one range"
    );
    let last = last_operator(core, plus, goto)?;
    core.write_code(at + 12, &(last as u32).to_le_bytes())?;
    Ok(core.report(&[at]))
}
