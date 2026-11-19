//! `sha1leak`, Windows: `sha1()`'s hex call goes to [`super::add_hex`], its file branch's `fclose` to
//! [`close_stub`].

use super::{AddHex, FOUND, Found, close_stub};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

/// push 14h ; lea ecx,[ebp-6Ch] (the SHA-1 state) ; call finish ; push eax ; lea ecx,[ebp-84h] (the editor) ;
/// call AddHex
const DIGEST: &str = "6A 14 8D 4D 94 E8 ?? ?? ?? ?? 50 8D 8D 7C FF FF FF E8";
const DIGEST_HEX: usize = 17;
/// The file branch (edi = the buffer, esi = the file): push 1000h ; call [malloc] ; ... ; (read loop) ;
/// push esi ; call [fclose] ; add esp,4 ; jmp
const FILE: &str = "68 00 10 00 00 FF 15 ?? ?? ?? ?? 8B 1D ?? ?? ?? ?? 8B F8 56 FF D3 83 C4 08 85 C0 75 ?? 56 68 00 10 \
                    00 00 6A 01 57 FF 15 ?? ?? ?? ?? 83 C4 10 8D 4D 94 50 57 E8 ?? ?? ?? ?? 56 FF D3 83 C4 04 85 C0 74 \
                    ?? 56 FF 15 ?? ?? ?? ?? 83 C4 04 EB";
const FILE_CLOSE: usize = 68;

close_stub!("edi");

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let digest = core.find_unique(DIGEST, "sha1(): digest")?;
    let file = core.find_unique(FILE, "sha1(): file")?;
    let fclose = core.iat_slot("fclose")?;
    FOUND.commit(
        cx,
        Found {
            add_hex: Foreign::at(core.call_target(digest + DIGEST_HEX)?),
            fclose: Foreign::at(core.live_ptr(fclose)?),
            free: Foreign::at(core.live_ptr(core.iat_slot("free")?)?),
        },
    );
    core.retarget(digest + DIGEST_HEX + 1, super::add_hex as AddHex)?;
    core.call_iat_to(file + FILE_CLOSE, fclose, close_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[digest, file]))
}
