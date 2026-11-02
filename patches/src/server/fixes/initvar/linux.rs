//! `initvar`, Linux: `initial()`'s call to the built-in var defaults goes through a stub that sends text and numbers
//! to the var read.
//!
//! `initial()` gets the tag in `dl`, the id in `ecx`, the result address in `eax`, and the var on the stack. A tag
//! without vars ends in a call to the defaults, cdecl (result address, tag, id, var) with `ret 4`. The var read has
//! the same signature, so the stub tail-jumps to either. It holds no frame, so the read's C++ exception unwinds into
//! `initial()`.

use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::server::fixes::minmax::value_tags;

/// `initial()`: push ebp,edi,esi,ebx ; sub esp,2Ch ; mov [esp+4],eax ; mov eax,edx ; mov edx,ecx ; mov edi,edx ;
/// mov ecx,eax ; cmp al,31h ; jbe a ; lea esi,[ecx-42h] ; mov ebx,esi ; cmp bl,10h ; ja d ; mov ebp,180C1h ;
/// bt ebp,esi ; jb b ; (d:) mov al,cl ; push [esp+40h] ; push edi ; push eax ; push [esp+10h] ; call defaults.
const INITIAL: &str = "55 57 56 53 83 EC 2C 89 44 24 04 89 D0 89 CA 89 D7 89 C1 3C 31 76 ?? 8D 71 BE 89 F3 80 FB 10 \
                       77 ?? BD C1 80 01 00 0F A3 F5 72 ?? 88 C8 FF 74 24 40 57 50 FF 74 24 10 E8 ?? ?? ?? ??";
/// Its `call defaults`.
const DEFAULTS_CALL: usize = 55;
/// The var read's (GetVariable's) tag switch: `movzx eax,dl ; movd [ebp-58h],xmm0 ; cmp al,58h ; ja`. Matched alone,
/// as `getvar` rewrites the loads before it; other functions' switches are candidates, not relied on.
const SWITCH: &str = "0F B6 C2 66 0F 7E 45 A8 3C 58 77";
/// [`SWITCH`]'s offset in the var read, which has an ebp frame: result address [ebp+8], value [ebp+0Ch], var
/// [ebp+14h].
const SWITCH_AT: usize = 0x21;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(INITIAL, "initial() entry")?;
    let call = f + DEFAULTS_CALL;
    let defaults = core.call_target(call)?;
    let read = core.probe_unique(SWITCH, "var read", |s| {
        core.is_at(s - SWITCH_AT, "55 89 E5 57 56 53 83 EC 5C").unwrap_or(false)
    })? - SWITCH_AT;
    // It must pop the result address as the defaults do: a `ret 4` close by.
    let ret4 = core.find_bytes_in(read, 0x300, &[0xC2, 0x04, 0x00])?;
    ensure!(!ret4.is_empty(), "var read does not pop its result address");
    let tags = value_tags(core)?;
    let stub = Stub::new()
        .bytes(&[0x80, 0x7C, 0x24, 0x08, tags.text]) // cmp byte [esp+8],TEXT
        .jcc(Cond::Equal, read) // je read
        .bytes(&[0x80, 0x7C, 0x24, 0x08, tags.number]) // cmp byte [esp+8],NUMBER
        .jcc(Cond::Equal, read) // je read
        .jmp(defaults) // jmp defaults
        .exec()?;
    core.call(call, 5, stub)?;
    Ok(format!("{}, var read {}", core.report(&[f]), core.report(&[read])))
}
