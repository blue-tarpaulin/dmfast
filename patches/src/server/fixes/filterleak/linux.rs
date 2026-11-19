//! `filterleak`, Linux: the filter var write's chain copy call goes to [`super::copy`].

use super::{FOUND, Found};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// In the filter var write: push [ebp-7Ch] (the chain) ; call copy ; mov esi,eax ; add esp,10h ; test eax,eax ; je
const WRITE: &str = "FF 75 84 E8 ?? ?? ?? ?? 89 C6 83 C4 10 85 C0 0F 84";
const WRITE_CALL: usize = 3;
/// The copy: push edi ; push esi ; push ebx ; mov edi,[esp+10h] ; sub esp,0Ch ; push 0Ch ; call malloc ;
/// add esp,10h ; mov ebx,eax ; test eax,eax ; je ; mov dword [eax+4],0 ; movzx eax,word [edi] ; mov [ebx],ax
const COPY_IDA: &str = "57 56 53 8B 7C 24 10 83 EC 0C 6A 0C E8 ?? ?? ?? ?? 83 C4 10 89 C3 85 C0 74 ?? C7 40 04 00 00 00 00 \
                        0F B7 07 66 89 03";
/// The chain interning's discards of a chain that exists or is empty: sub esp,0Ch ; push edi ;
/// call release_array ; mov eax,[free_list] ; add esp,10h ; mov [free_list],edi ; mov [edi+8],eax
const DISCARD: &str = "83 EC 0C 57 E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 10 89 3D ?? ?? ?? ?? 89 47 08";
const DISCARD_HEAD: usize = 10;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let write = core.find_unique(WRITE, "filter var write: chain copy")?;
    let copy = core.follow_call(write + WRITE_CALL, COPY_IDA, "filter chain copy")?;
    let discards = core.find_all(DISCARD)?;
    let head = core.ptr_at(discards.first_or("filter chain interning: discard")? + DISCARD_HEAD)?;
    for &d in &discards {
        ensure!(core.ptr_at(d + DISCARD_HEAD)? == head, "filter chain interning: discards use different lists");
    }
    FOUND.commit(cx, Found { copy: Foreign::at(copy), free: Foreign::at(libc::free as *const ()), head });
    core.retarget(write + WRITE_CALL + 1, super::copy as extern "C" fn(usize) -> usize)?;
    Ok(core.report(&[write]))
}
