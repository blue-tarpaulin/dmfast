//! `await`, Linux: the resume holds the proc state in ebx, the sleeper in esi, and the value in xmm0.

use crate::base::code::{Cond, Stub};
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// `Byond_Return`'s sleeper lookup, at the await sleeper: cmp byte [edx],type ; jne next ; cmp [ecx+x],esi ;
/// jne next ; (sub esp,0Ch ; push 1 ;) mov edx,[esp+x] ; mov ecx,[esp+x] (the value) ; call resume (eax: the index)
const LOOKUP: [&str; 2] = [
    "80 3A ?? 75 ?? 39 71 ?? 75 ?? 83 EC 0C 6A 01 8B 54 24 ?? 8B 4C 24 ?? E8",
    "80 3A ?? 75 ?? 39 71 ?? 75 ?? 8B 54 24 ?? 8B 4C 24 ?? E8",
];
/// The resume's test whether to push: and edx,4 (the sleeper's flags) ; jne skip ; mov ecx,[ebx] ;
/// mov r,[ebx+stack] ; mov r,[ecx+x] ; mov [esp+x],r ; test r,r ; je ; movzx r,word [ebx+pos]
pub(super) const HOOK: &str = "83 E2 04 0F 85 ?? ?? ?? ?? 8B 0B 8B ?? ?? 8B ?? ?? ?? ?? ?? 89 ?? 24 ?? 85 ?? 74 ?? 0F B7 \
                               ?? ??";
/// The `and ; jne` the hook replaces.
pub(super) const HOOK_LEN: usize = 9;
pub(super) const STACK: usize = 13;
pub(super) const POS: usize = 31;
/// The flags' load: movzx edx,byte [esi+1]
const FLAGS: &str = "0F B6 56 01";

/// The await sleepers' type and the resume.
pub(super) fn lookup(core: &Image) -> Result<(u8, usize)> {
    for pat in LOOKUP {
        if let Ok(at) = core.find_unique(pat, "await sleeper lookup") {
            return Ok((core.u8_at(at + 2)?, core.call_target(at + pat.split(' ').count() - 1)?));
        }
    }
    bail!("await sleeper lookup: not found")
}

/// Checks the resume loads the flags from the sleeper in esi, and the pushes' operands are [ebx+disp8].
pub(super) fn check(core: &Image, resume: usize, site: usize) -> Result<()> {
    core.find_unique_in(resume, site - resume, FLAGS, "await resume flags")?;
    let ebx = |at| core.u8_at(at).map(|m| m & 0xC7 == 0x43);
    ensure!(ebx(site + STACK - 1)? && ebx(site + POS - 1)?, "await resume push: not on ebx's stack");
    Ok(())
}

/// The hook: the test and jump, then `pop(state, sleeper)` with xmm0 kept and the stack 16-byte aligned, as at the
/// site.
pub(super) fn stub(core: &Image, site: usize, pop: extern "C" fn(usize, usize)) -> Result<usize> {
    let skip = core.jcc_at(site + 3, Cond::NotEqual)?.target;
    Stub::new()
        .bytes(&[0x83, 0xE2, 0x04]) // and edx,4
        .jcc(Cond::NotEqual, skip) // jne skip
        .bytes(&[0x83, 0xEC, 0x10]) // sub esp,10h
        .bytes(&[0x66, 0x0F, 0xD6, 0x44, 0x24, 0x08]) // movq [esp+8],xmm0
        .bytes(&[0x89, 0x74, 0x24, 0x04]) // mov [esp+4],esi
        .bytes(&[0x89, 0x1C, 0x24]) // mov [esp],ebx
        .call(pop) // call pop
        .bytes(&[0xF3, 0x0F, 0x7E, 0x44, 0x24, 0x08]) // movq xmm0,[esp+8]
        .bytes(&[0x83, 0xC4, 0x10]) // add esp,10h
        .jmp(site + HOOK_LEN) // jmp back
        .exec()
}
