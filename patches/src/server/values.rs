//! BYOND's release of an array of values, which features that let go of values on the interpreter's stack or in a
//! buffer call.

use crate::base::Result;
use crate::base::feature::Ctx;

/// A value as BYOND stores it on the stack and in buffers: its type and data.
pub(crate) type Slot = [u32; 2];

/// Releases each of `count` values from `first` on and nulls its slot, last first (cdecl).
pub(crate) type ReleaseFn = unsafe extern "C" fn(*mut Slot, u32);

/// The release loop's start: push ebp ; mov ebp,esp ; push esi ; mov esi,[ebp+0Ch] (count) ; test esi,esi ; je ;
/// push edi ; mov edi,[ebp+8] (first) ; ... ; loop: mov eax,[edi+esi*8-4] ; dec esi ; push eax ; mov ecx,[edi+esi*8] ;
/// push ecx ; mov dword [edi+esi*8],0 ; mov dword [edi+esi*8+4],0 ; call DecRef.
#[cfg(windows)]
const RELEASE: &str = "55 8B EC 56 8B 75 0C 85 F6 74 ?? 57 8B 7D 08 ?? 8B 44 F7 FC 4E 50 8B 0C F7 51 C7 04 F7 00 00 00 00 \
                       C7 44 F7 04 00 00 00 00 E8";
/// push esi ; push ebx ; sub esp,14h ; mov eax,[esp+24h] (count) ; mov esi,[esp+20h] (first) ; test eax,eax ; je ;
/// lea ebx,[eax-1], then [`RELEASE_LOOP`] after alignment padding.
#[cfg(target_os = "linux")]
const RELEASE: &str = "56 53 83 EC 14 8B 44 24 24 8B 74 24 20 85 C0 74 ?? 8D 58 FF";
/// movq xmm0,[esi+ebx*8] ; sub esp,8 ; mov dword [esi+ebx*8],0 ; mov dword [esi+ebx*8+4],0 ; sub ebx,1
#[cfg(target_os = "linux")]
const RELEASE_LOOP: &str = "F3 0F 7E 04 DE 83 EC 08 C7 04 DE 00 00 00 00 C7 44 DE 04 00 00 00 00 83 EB 01";

/// Where BYOND's release of an array of values is ([`ReleaseFn`]).
pub(crate) fn release(cx: &Ctx) -> Result<usize> {
    cx.shared("value array release", |cx| {
        let core = cx.core();
        let f = core.find_unique(RELEASE, "value array release")?;
        #[cfg(target_os = "linux")]
        core.find_unique_in(f + RELEASE.split(' ').count(), 0x30, RELEASE_LOOP, "value array release loop")?;
        Ok(f)
    })
    .copied()
}
