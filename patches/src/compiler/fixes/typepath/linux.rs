//! `typepath`, Linux: the check `lea edx,[eax-0ADh] ; cmp edx,bound` gets the bound of `.index`.

use super::last_operator;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail};

/// The name table, from its interning loop: push [ebx*4+names+4] ; add ebx,1 ; push esi ; call intern ;
/// add esp,10h ; cmp ebx,count ; jne ; cmp [esi+88h],count (two inlined copies, one table).
fn names(core: &Image) -> Result<usize> {
    let hits = core.find_all(
        "FF 34 9D ?? ?? ?? ?? 83 C3 01 56 E8 ?? ?? ?? ?? 83 C4 10 81 FB ?? ?? ?? ?? 75 E2 81 BE 88 00 00 00",
    )?;
    let Some(&first) = hits.as_slice().first() else { bail!("name table not found") };
    let table = core.ptr_at(first + 3)?;
    for &h in hits.iter() {
        if core.ptr_at(h + 3)? != table {
            bail!("name table: the interning loops read different tables");
        }
    }
    Ok(table - 4)
}

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
    let [b] = core.array::<1>(cmp)?;
    let bound = last_operator(core, names(core)?, plus, plus + b as usize)? - plus;
    // cmp edx,imm8 is sign-extended.
    if bound > 0x7F {
        bail!("id bound {bound:#x} does not fit the check");
    }
    core.write_code(cmp, &[bound as u8])?;
    Ok(format!("at +{:#x}", at - core.base))
}
