//! `varoverride`, Windows: a Rust hook on the recorder's entry.

use super::{RECORD, record};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The recorder: push ebp ; mov ebp,esp ; sub esp,10h ; mov eax,[cookie] ; xor eax,ebp ; mov [ebp-4],eax ;
/// mov eax,[ebp+8] ; mov ecx,[list] ; mov [ebp-10h],eax ; mov eax,[ebp+0Ch] ; mov [ebp-0Ch],eax ;
/// movzx eax,word [start] ; mov [ebp-8],ax ; mov ax,[ebp+10h] ; mov [ebp-6],ax ; mov [start],ax ; test ecx,ecx ;
/// je done ; lea eax,[ebp-10h] ; push eax ; call add-or-replace (by name, the entry's first dword).
const RECORDER: &str = "55 8B EC 83 EC 10 A1 ?? ?? ?? ?? 33 C5 89 45 FC 8B 45 08 8B 0D ?? ?? ?? ?? 89 45 F0 8B 45 0C \
                        89 45 F4 0F B7 05 ?? ?? ?? ?? 66 89 45 F8 66 8B 45 10 66 89 45 FA 66 A3 ?? ?? ?? ?? 85 C9 74 ?? \
                        8D 45 F0 50 E8";
/// The start global, read and written.
const START_READ: usize = 0x25;
const START_WRITE: usize = 0x37;
/// push ebp ; mov ebp,esp ; sub esp,10h
const ENTRY: usize = 6;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(RECORDER, "init proc recorder")?;
    ensure!(core.ptr_at(f + START_READ)? == core.ptr_at(f + START_WRITE)?, "init proc recorder: two start globals");
    RECORD.set(core.hook(f, ENTRY, record as extern "C" fn(u32, u32, u32))?);
    Ok(core.report(&[f]))
}
