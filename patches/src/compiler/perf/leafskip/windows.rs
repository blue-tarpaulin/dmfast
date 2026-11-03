//! `leafskip`, Windows: the pass function (cdecl: node, mode) jumps to a stub built for the flag offset, bit, and
//! count offset its handlers test ([`crate::compiler::layout`]). Bit 2 is only set with or after bit 1 and cleared
//! with it, so a finished node also counts as resolved where a pass tests bit 1.

use super::verify::{FOUND, Found, ORIGINAL, verify_visit};
use crate::base::code::{Cond, Stub, trampoline};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// SEH prologue ; sub esp,18h ; ... ; mov esi,[ebp+0Ch] ; xor ebx,ebx ; mov [ebp+0Ch],ebx ; mov edi,[ebp+8] ;
/// movzx eax,byte [edi+4]
const PASS: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 18 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 \
                    8D 45 F4 64 A3 00 00 00 00 8B 75 0C 33 DB 89 5D 0C 8B 7D 08 0F B6 47 04";
/// The moved entry: push ebp ; mov ebp,esp ; push -1 (position-independent).
const ENTRY: usize = 5;
/// A handler's finished test (edi = node; twice in 515-516): `test byte [edi+flags],bit ; jne done ;
/// movzx ecx,word [edi+count] ; test cx,cx ; je ; mov edx,[edi+children]`.
const TEST: &str = "F6 47 ?? ?? 0F 85 ?? ?? ?? ?? 0F B7 4F ?? 66 85 C9 74 ?? 8B 57";
/// The shared tail: or dword [edi+14h],3 ; inc dword [finished] ; mov byte [progress],1
const TAIL: &str = "83 4F 14 03 FF 05 ?? ?? ?? ?? C6 05 ?? ?? ?? ?? 01";

/// Front of the pass function: `mov eax,[esp+4] ; test byte [eax+flags],bit ; jz orig ; cmp word [eax+count],0 ;
/// jne orig ; xor eax,eax ; ret ; orig: jmp original`.
fn visit_stub(flags: u8, bit: u8, count: u8, original: usize) -> Result<usize> {
    let mut s = Stub::new();
    let orig = s.label();
    s.bytes(&[0x8B, 0x44, 0x24, 0x04])
        .bytes_u32(&[0xF6, 0x80], u32::from(flags), &[bit])
        .jcc8(Cond::Zero, orig)
        .bytes_u32(&[0x66, 0x83, 0xB8], u32::from(count), &[0x00])
        .jcc8(Cond::NotEqual, orig)
        .bytes(&[0x33, 0xC0, 0xC3])
        .bind(orig)
        .jmp(original)
        .exec()
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(PASS, "compile pass function")?;
    // All the finished tests must agree.
    let fields = core
        .find_in(f, 0x600, TEST)?
        .iter()
        .map(|&c| core.array::<14>(c).map(|b| [b[2], b[3], b[13]]))
        .collect::<Result<Vec<_>>>()?;
    let [flags, bit, count] = match fields[..] {
        [first, ref rest @ ..] if first[1] != 0 && rest.iter().all(|&r| r == first) => first,
        _ => bail!("compile pass function: finished-flag checks {fields:?}"),
    };
    if verify() {
        let t = core.find_unique_in(f, 0x800, TAIL, "compile pass function: progress store")?;
        let (finished, progress) = (core.ptr_at(t + 6)?, core.ptr_at(t + 12)?);
        FOUND.commit(cx, Found { flags: flags.into(), bit, count: count.into(), progress, finished });
        ORIGINAL.set(core.hook(f, ENTRY, verify_visit as unsafe extern "C" fn(usize, u32) -> u32)?);
        return Ok(format!("{}, verify", core.report(&[f])));
    }
    // The stub jumps to the original, so the original comes first: `Image::hook` wants the hook first.
    core.untouched(&[(f, ENTRY)])?;
    let stub = visit_stub(flags, bit, count, trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?)?;
    core.jump(f, ENTRY, stub)?;
    Ok(core.report(&[f]))
}
