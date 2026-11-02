//! `urlcrlf`, Linux: the newline test's `je crlf` becomes NOPs.

use crate::base::Result;
use crate::base::feature::Ctx;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // cmp bl,0Ah ; je crlf ; cmp bl,0FFh ; jne generic ; cmp byte [edx+1],0
    let at = core.find_unique("80 FB 0A 0F 84 ?? ?? ?? ?? 80 FB FF 75 ?? 80 7A 01 00", "URL encoder newline test")?;
    core.never_jump(at + 3)?;
    Ok(core.report(&[at]))
}
