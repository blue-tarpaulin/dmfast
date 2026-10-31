//! `cmperr`, Windows: the handlers share one error block, so their `jne skip` all go to one stub.

use super::HANDLERS;
use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// `push right value ; push right type ; push op text ; push left value ; push left type ;
/// push "type mismatch: cannot compare" ; call format`
const ERROR: &str = "FF B5 EC FD FF FF FF B5 E8 FD FF FF 68 ?? ?? ?? ?? FF 74 CA FC FF 74 CA F8 68 ?? ?? ?? ?? E8";
/// `cmp al,2Ah ; jne skip ; mov edx,[right value] ; mov eax,[right type] ; jmp number_compare`
const HANDLER: &str = "3C 2A 0F 85 ?? ?? ?? ?? 8B 95 EC FD FF FF 8B 85 E8 FD FF FF E9";
/// The handler's fetch of the left operand, which the error block reads through edx (the value stack) and ecx (its
/// top): `mov eax,[state] ; movzx ecx,word [eax+pos] ; mov edx,[eax+base]` (12 bytes, absolute addresses only).
const FETCH: usize = 0x19;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let error = core.find_unique(ERROR, "cannot compare error block")?;
    let sites = core.find_all(HANDLER)?;
    if sites.len() < HANDLERS {
        bail!("comparison handlers: expected at least {HANDLERS}, found {}", sites.len());
    }
    let first = sites.as_slice()[0];
    let skip = core.rel32(first + 4)?;
    for &s in sites.iter() {
        if core.rel32(s + 4)? != skip {
            bail!("comparison handlers do not share one skip tail");
        }
    }
    let fetch = core.array::<12>(first + FETCH)?;
    if fetch[0] != 0xA1 || fetch[5..8] != [0x0F, 0xB7, 0x48] || fetch[9..11] != [0x8B, 0x50] {
        bail!("comparison handler: left operand fetch not as expected");
    }
    let stub = exec_code(fetch.len() + 5, |p| {
        let mut v = fetch.to_vec();
        v.extend(branch(0xE9, p + v.len(), error, 5)); // jmp error
        v
    })?;
    for &s in sites.iter() {
        core.write_code(s + 4, &(stub.wrapping_sub(s + 8) as u32).to_le_bytes())?;
    }
    Ok(format!("{} handlers", sites.len()))
}
