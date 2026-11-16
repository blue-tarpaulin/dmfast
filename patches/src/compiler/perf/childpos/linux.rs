//! `childpos`, Linux: libbyond's child position function jumps to [`position`].

use super::position;
use crate::base::Result;
use crate::base::feature::Ctx;

/// The whole function (cdecl, node at `[esp+4]`): parent at `+0x14`, count at `+0x1e`, children at `+0x18`.
const POSITION: &str = "53 8B 54 24 08 85 D2 74 27 8B 42 14 85 C0 74 20 0F B7 48 1E 66 85 C9 74 17 8B 58 18 31 C0 EB 08 \
                        83 C0 01 66 39 C1 76 08 39 14 83 75 F3 5B C3 90 B8 FF FF FF FF 5B C3";

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(POSITION, "child position")?;
    core.jump(f, 5, position as extern "C" fn(usize) -> i32)?;
    Ok(core.report(&[f]))
}
