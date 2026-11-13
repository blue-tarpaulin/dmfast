//! `varoverride`, Linux: the var write's tail jump to the recorder goes to the Rust hook (the recorder's entry reads
//! an absolute address, relocated at load, so it can't be moved into a trampoline).

use super::{RECORD, record};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The recorder: sub esp,1Ch ; movzx eax,word [start] ; mov edx,[esp+28h] ; movd xmm0,[esp+20h] ;
/// movd xmm1,[esp+24h] ; mov ecx,edx ; mov [start],dx ; shl ecx,10h ; punpckldq xmm0,xmm1 ; or eax,ecx ;
/// movq [esp],xmm0 ; mov [esp+8],eax ; mov eax,[list] ; test eax,eax ; je done ; sub esp,8 ; lea edx,[esp+8] ;
/// push edx ; push eax ; call add-or-replace (by name, the entry's first dword).
const RECORDER: &str = "83 EC 1C 0F B7 05 ?? ?? ?? ?? 8B 54 24 28 66 0F 6E 44 24 20 66 0F 6E 4C 24 24 89 D1 66 89 15 \
                        ?? ?? ?? ?? C1 E1 10 66 0F 62 C1 09 C8 66 0F D6 04 24 89 44 24 08 A1 ?? ?? ?? ?? 85 C0 74 ?? \
                        83 EC 08 8D 54 24 08 52 50 E8";
/// The start global, read and written.
const START_READ: usize = 6;
const START_WRITE: usize = 0x1F;
/// The var write on a recording `src`, passing (name, init proc, pc + 1) in its own argument slots:
/// movzx eax,word [edx+14h] ; add eax,1 ; movzx eax,ax ; mov [ebp+10h],eax ; mov eax,[edx] ; mov eax,[eax] ;
/// mov [ebp+8],name ; mov [ebp+0Ch],eax ; lea esp,[ebp-0Ch] ; pop ebx ; pop esi ; pop edi ; pop ebp ; jmp recorder
const WRITE: &str = "0F B7 42 14 83 C0 01 0F B7 C0 89 45 10 8B 02 8B 00 89 ?? 08 89 45 0C 8D 65 F4 5B 5E 5F 5D E9";
/// [`WRITE`]'s jump's rel32.
const WRITE_JUMP: usize = 0x1F;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(RECORDER, "init proc recorder")?;
    ensure!(core.ptr_at(f + START_READ)? == core.ptr_at(f + START_WRITE)?, "init proc recorder: two start globals");
    let write =
        core.probe_unique(WRITE, "init proc recording", |s| core.rel32(s + WRITE_JUMP).is_ok_and(|t| t == f))?;
    RECORD.set(f);
    core.retarget(write + WRITE_JUMP, record as extern "C" fn(u32, u32, u32))?;
    Ok(format!("{}, recorder {}", core.report(&[write]), core.report(&[f])))
}
