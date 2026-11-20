//! `inputop`, Linux: the null first arg is written through a pointer to the arg array.

use super::point_at;
use crate::base::Result;
use crate::base::feature::Ctx;

/// mov eax,[ebp+stale] ; mov edx,[context] ; sub esp,8 ; mov esi,[ebp+..] ; mov dword [eax],0 ;
/// mov dword [eax+4],0 (the first arg's null) ; movzx eax,word [edx+42h] ; mov ecx,[edx+3Ch]
const ROUTE: &str = "8B 85 ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 83 EC 08 8B B5 ?? ?? ?? ?? C7 00 00 00 00 00 C7 40 04 00 00 00 \
                     00 0F B7 42 42 8B 4A 3C";
/// Later, the arg array copied into the call's block: movq xmm0,[ebp+args] ; mov eax,[ebp+..] ; movq [eax],xmm0
const COPY: &str = "F3 0F 7E 85 ?? ?? ?? ?? 8B 85 ?? ?? ?? ?? 66 0F D6 00";

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(ROUTE, "overloaded input: first arg")?;
    let copy = core.find_unique_in(at, 0xA0, COPY, "overloaded input: arg array")?;
    point_at(core, at, copy + 4)
}
