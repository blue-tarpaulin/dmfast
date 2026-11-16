//! `childpos`, Windows: byondcore's child position function jumps to [`position`].

use super::position;
use crate::base::Result;
use crate::base::feature::Ctx;

/// The whole function (cdecl, node at `[ebp+8]`): parent at `+0x18`, count at `+0x22`, children at `+0x1c`.
const POSITION: &str = "55 8B EC 8B 55 08 56 57 85 D2 74 25 8B 4A 18 85 C9 74 1E 0F B7 71 22 33 FF 33 C0 66 3B FE 73 11 \
                        8B 79 1C 0F B7 C8 39 14 8F 74 0D 40 66 3B C6 72 F2 5F 83 C8 FF 5E 5D C3 5F 8B C1 5E 5D C3";

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(POSITION, "child position")?;
    core.jump(f, 5, position as extern "C" fn(usize) -> i32)?;
    Ok(core.report(&[f]))
}
