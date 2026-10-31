//! `urlcrlf`, Windows: the newline test's `jne generic` becomes `jmp generic`.

use crate::base::Result;
use crate::base::feature::Ctx;

/// How far into the encoder the newline test is searched for.
const SPAN: usize = 0x200;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.export(c"?AddUrlEncoded@StringEditor@@QAEXPBD@Z")?;
    // cmp bl,0Ah ; jne generic ; push 6 ; mov ecx,esi ; call IncBufLen
    let at = core.find_unique_in(f, SPAN, "80 FB 0A 75 ?? 6A 06 8B CE E8", "URL encoder newline test")?;
    core.write_code(at + 3, &[0xEB])?;
    Ok(format!("at +{:#x}", at - core.base))
}
