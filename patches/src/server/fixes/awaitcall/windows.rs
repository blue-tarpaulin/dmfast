//! `await`, Windows: the resume holds the proc state in ebx and the sleeper in edi.

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::image::Image;

/// `Byond_Return`'s sleeper lookup, at the await sleeper: cmp byte [ecx],type ; jne next ; cmp [eax+x],edi ;
/// je found ; next: ... ; found: push 1 ; push [ebp+14h] ; push [ebp+10h] (the value) ; push edx (the index) ;
/// call resume
const LOOKUP: &str = "80 39 ?? 75 ?? 39 78 ?? 74 ?? 42 83 C1 ?? 3B D6 72 ?? 5F 32 C0 5E 5D C3 6A 01 FF 75 14 FF 75 10 \
                      52 E8";
/// The resume's test whether to push: test cl,4 (the sleeper's flags) ; jne skip ; mov ecx,[ebx] ; ... ;
/// movzx edx,word [ebx+pos] ; ... ; push [ebx+stack]
pub(super) const HOOK: &str = "F6 C1 04 0F 85 ?? ?? ?? ?? 8B 0B 8B 81 ?? ?? ?? ?? 85 C0 74 ?? 0F B7 53 ?? 3B C2 77 ?? \
                               8D 42 01 EB ?? 8B 81 ?? ?? ?? ?? 83 F8 0A 75 ?? 0F B7 43 ?? 40 50 FF 73 ??";
/// The `test ; jne` the hook replaces.
pub(super) const HOOK_LEN: usize = 9;
pub(super) const POS: usize = 24;
pub(super) const STACK: usize = 53;
/// The flags' load: mov cl,[edi+1]
const FLAGS: &str = "8A 4F 01";

/// The await sleepers' type and the resume.
pub(super) fn lookup(core: &Image) -> Result<(u8, usize)> {
    let at = core.find_unique(LOOKUP, "await sleeper lookup")?;
    Ok((core.u8_at(at + 2)?, core.call_target(at + LOOKUP.split(' ').count() - 1)?))
}

/// Checks the resume loads the flags from the sleeper in edi.
pub(super) fn check(core: &Image, resume: usize, site: usize) -> Result<()> {
    core.find_unique_in(resume, site - resume, FLAGS, "await resume flags")?;
    Ok(())
}

/// The hook: the test and jump, then `pop(state, sleeper)`.
pub(super) fn stub(core: &Image, site: usize, pop: extern "C" fn(usize, usize)) -> Result<usize> {
    let skip = core.jcc_at(site + 3, Cond::NotEqual)?.target;
    Stub::new()
        .bytes(&[0xF6, 0xC1, 0x04]) // test cl,4
        .jcc(Cond::NotEqual, skip) // jne skip
        .bytes(&[0x57]) // push edi
        .bytes(&[0x53]) // push ebx
        .call(pop) // call pop
        .bytes(&[0x83, 0xC4, 0x08]) // add esp,8
        .jmp(site + HOOK_LEN) // jmp back
        .exec()
}
