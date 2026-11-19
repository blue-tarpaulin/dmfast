//! `sha1leak`, Linux: `sha1()`'s two hex calls go to [`super::add_hex`], its file branch's `fclose` to
//! [`close_stub`].

use super::{AddHex, FOUND, Found, close_stub};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// `sha1()`'s two copies: call finish ; (mov esi,[ebp-..]) ; add esp,0Ch ; push 14h ; push eax ;
/// push the editor ; call AddHex
const DIGESTS: [(&str, usize); 2] = [
    ("E8 ?? ?? ?? ?? 8B B5 ?? ?? ?? ?? 83 C4 0C 6A 14 50 56 E8", 18),
    ("E8 ?? ?? ?? ?? 83 C4 0C 6A 14 50 FF B5 ?? ?? ?? ?? E8", 17),
];
/// The file branch (esi = the buffer, ebx = the file): push 1000h ; call malloc ; ... ; (read loop) ;
/// sub esp,0Ch ; push ebx ; call fclose ; jmp
const FILE: &str = "68 00 10 00 00 E8 ?? ?? ?? ?? 83 C4 10 89 C6 EB ?? 53 68 00 10 00 00 6A 01 56 E8 ?? ?? ?? ?? 83 C4 0C \
                    50 56 FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 83 EC 0C 53 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? 83 EC \
                    0C 53 E8 ?? ?? ?? ?? E9";
const FILE_CLOSE: usize = 70;

close_stub!("esi");

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut hex = Vec::new();
    for (ida, call) in DIGESTS {
        hex.push(core.find_unique(ida, "sha1(): digest")? + call);
    }
    let add_hex = core.call_target(hex[0])?;
    ensure!(core.call_target(hex[1])? == add_hex, "sha1(): copies hex with different functions");
    let file = core.find_unique(FILE, "sha1(): file")?;
    FOUND.commit(
        cx,
        Found {
            add_hex: Foreign::at(add_hex),
            fclose: Foreign::at(core.call_target(file + FILE_CLOSE)?),
            free: Foreign::at(libc::free as *const ()),
        },
    );
    for &h in &hex {
        core.retarget(h + 1, super::add_hex as AddHex)?;
    }
    core.retarget(file + FILE_CLOSE + 1, close_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[hex[0], hex[1], file]))
}
