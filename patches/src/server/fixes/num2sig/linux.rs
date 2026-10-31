//! `num2sig`, Linux: libbyond inlines the formatter into each NUM2TEXT handler, so only the two-argument handler's
//! `sprintf` call is pointed at [`sig_g`].

use super::{SPRINTF, sig_g};
use crate::base::Result;
use crate::base::feature::Ctx;

/// Two-argument num2text: `xor eax,eax ; test ebx,ebx ; cmovns eax,ebx ; mov esi,eax ; mov eax,64h ; cmp esi,eax ;
/// cmovg esi,eax ; sub esp,0Ch ; lea eax,[esi+0Ah] ; push eax ; call alloc ; add esp,10h ; mov ebx,eax ;
/// test eax,eax ; je oom ; fld [value] ; sub esp,14h ; fstp qword [esp] ; push esi (digits) ; push "%.*g" ;
/// push eax (buffer) ; call sprintf ; add esp,20h`
const HANDLER: &str = "31 C0 85 DB 0F 49 C3 89 C6 B8 64 00 00 00 39 C6 0F 4F F0 83 EC 0C 8D 46 0A 50 E8 ?? ?? ?? ?? \
                       83 C4 10 89 C3 85 C0 0F 84 ?? ?? ?? ?? D9 85 ?? ?? ?? ?? 83 EC 14 DD 1C 24 56 68 ?? ?? ?? ?? 50 \
                       E8 ?? ?? ?? ?? 83 C4 20";
/// Its `call sprintf`.
const CALL: usize = 63;

type SigG = unsafe extern "C" fn(*mut u8, *const u8, i32, f64) -> i32;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let call = core.find_unique(HANDLER, "num2text two-argument %g call")? + CALL;
    SPRINTF.set(core.rel32(call + 1)?);
    core.call(call, 5, sig_g as SigG)?;
    Ok(format!("at +{:#x}", call - core.base))
}
