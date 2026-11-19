//! `filterleak`, Windows: the filter var write's chain copy call goes to [`super::copy`].

use super::{FOUND, Found};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

/// In the filter var write: push [ebp-18h] (the chain) ; mov dword [ebp-4],2 ; call copy ; mov edi,eax ;
/// add esp,4 ; test edi,edi
const WRITE: &str = "FF 75 E8 C7 45 FC 02 00 00 00 E8 ?? ?? ?? ?? 8B F8 83 C4 04 85 FF";
const WRITE_CALL: usize = 10;
/// The copy: push ebp ; mov ebp,esp ; push ecx ; push esi ; push 0Ch ; call [malloc] ; mov esi,eax ; add esp,4 ;
/// test esi,esi ; jne ; ... ; mov dword [esi+4],0 ; movzx eax,word [edi] ; mov [esi],ax
const COPY_IDA: &str = "55 8B EC 51 56 6A 0C FF 15 ?? ?? ?? ?? 8B F0 83 C4 04 85 F6 75 05 5E 8B E5 5D C3 57 8B 7D 08 \
                        C7 46 04 00 00 00 00 0F B7 07 66 89 06";
/// The chain interning's discard of a chain that exists: push edi ; call release_array ; mov eax,[free_list] ;
/// add esp,4
const DISCARD: &str = "57 E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 04 89 47 08 89 3D";
const DISCARD_HEAD: usize = 7;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let write = core.find_unique(WRITE, "filter var write: chain copy")?;
    let copy = core.follow_call(write + WRITE_CALL, COPY_IDA, "filter chain copy")?;
    let discard = core.find_unique(DISCARD, "filter chain interning: discard")?;
    FOUND.commit(
        cx,
        Found {
            copy: Foreign::at(copy),
            free: Foreign::at(core.live_ptr(core.iat_slot("free")?)?),
            head: core.ptr_at(discard + DISCARD_HEAD)?,
        },
    );
    core.retarget(write + WRITE_CALL + 1, super::copy as extern "C" fn(usize) -> usize)?;
    Ok(core.report(&[write]))
}
