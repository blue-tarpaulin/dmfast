//! `typepath`, Linux: the check `lea edx,[eax-0ADh] ; cmp edx,bound` gets the bound of `.index`.

use super::last_operator;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // 516: mov eax,[ebx] ; add esp,10h ; lea edx,[eax-0ADh] ; cmp edx,38h ; ja ok ; sub esp,8 ; push message
    let hits =
        core.find_all("8B 03 83 C4 10 8D 90 53 FF FF FF 83 FA 38 0F 87 ?? ?? ?? ?? 83 EC 08 68 ?? ?? ?? ?? 53 E8")?;
    let (at, lea, cmp) = if hits.is_empty() {
        // 515: ids are 8 lower and the layout inverted: lea edx,[eax-0A5h] ; cmp edx,36h ; jbe reject
        let at = core.find_unique(
            "8B 03 8D 90 5B FF FF FF 83 FA 36 0F 86 ?? ?? ?? ?? 80 7B 04 00",
            "path element id check (515)",
        )?;
        (at, at + 4, at + 10)
    } else {
        let at = hits.unique("path element id check")?;
        (at, at + 7, at + 13)
    };
    let plus = (core.u32_at(lea)? as i32).wrapping_neg() as usize;
    let b = core.u8_at(cmp)?;
    let bound = last_operator(core, plus, plus + b as usize)? - plus;
    // cmp edx,imm8 is sign-extended.
    ensure!(bound <= 0x7F, "id bound {bound:#x} does not fit the check");
    core.write_code(cmp, &[bound as u8])?;
    Ok(core.report(&[at]))
}
