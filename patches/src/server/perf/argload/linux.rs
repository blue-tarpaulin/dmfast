//! `argload`, Linux: rewrites of the three prologues, installed together or not at all.

use std::ffi::CStr;

use crate::base::code::Stub;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

const INCREF: &str = "55 89 E5 56 53 83 EC 20 F3 0F 7E 4D 08 66 0F 6F C1 66 0F 7E C9 66 0F 73 D0 20 66 0F 7E C0 \
                      80 F9 4A 77 ?? 0F B6 C9 FF 24 8D";
const DECREF: &str = "55 89 E5 56 53 83 EC 20 F3 0F 7E 4D 08 66 0F 6F C1 66 0F 7E C9 66 0F 73 D0 20 66 0F 7E C3 \
                      80 F9 4A 77 ?? 0F B6 C9 FF 24 8D";
const ASSIGN: &str = "83 EC 2C 8B 44 24 30 F3 0F 7E 44 24 34 F3 0F 7E 08 66 0F D6 00 66 0F D6 04 24 66 0F D6 4C 24 18 \
                      E8 ?? ?? ?? ?? F3 0F 7E 4C 24 18 66 0F D6 4C 24 30 83 C4 2C E9 ?? ?? ?? ??";

/// How far into the exported wrapper its call of the internal function may be.
const WRAPPER_SPAN: usize = 0x400;

/// IncRef/DecRef at +8: stock's `movq xmm1,[ebp+8]` split into ecx = lo and `hi` (a register number) = hi, as two
/// dword loads. xmm1 = lo|hi<<32 and xmm0 = hi as before, no flags touched (22 bytes).
fn refcount(hi: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend([0x8B, 0x4D, 0x08]); // mov ecx,[ebp+8]
    v.extend([0x8B, 0x45 | hi << 3, 0x0C]); // mov hi,[ebp+0Ch]
    v.extend([0x66, 0x0F, 0x6E, 0xC9]); // movd xmm1,ecx
    v.extend([0x66, 0x0F, 0x6E, 0xC0 | hi]); // movd xmm0,hi
    v.extend([0x66, 0x0F, 0x62, 0xC8]); // punpckldq xmm1,xmm0
    v.extend([0x0F, 0x1F, 0x40, 0x00]); // nop4
    v
}

/// `assign(Value *p, Value v)` at +7: the new value moves through ecx/edx and the old one goes straight into
/// the dead argument slot `[esp+30h]`, so both calls' loads are store-forwarded. The IncRef call is re-encoded
/// (return address +37 -> +42: same unwind rule); stock's `jmp DecRef` tail follows ([`ASSIGN_LEN`] bytes).
fn assign(incref: usize) -> Stub {
    let mut s = Stub::new();
    s.bytes(&[0x8B, 0x4C, 0x24, 0x34]) // mov ecx,[esp+34h]
        .bytes(&[0x8B, 0x54, 0x24, 0x38]) // mov edx,[esp+38h]
        .bytes(&[0xF3, 0x0F, 0x7E, 0x08]) // movq xmm1,[eax]
        .bytes(&[0x89, 0x08]) // mov [eax],ecx
        .bytes(&[0x89, 0x50, 0x04]) // mov [eax+4],edx
        .bytes(&[0x89, 0x0C, 0x24]) // mov [esp],ecx
        .bytes(&[0x89, 0x54, 0x24, 0x04]) // mov [esp+4],edx
        .bytes(&[0x66, 0x0F, 0xD6, 0x4C, 0x24, 0x30]) // movq [esp+30h],xmm1
        .call(incref) // call IncRef
        .bytes(&[0x0F, 0x1F, 0x80, 0x00, 0x00, 0x00, 0x00]); // nop7
    s
}
const ASSIGN_LEN: usize = 42;

/// Fails unless the exported `wrapper` calls `target` within its first [`WRAPPER_SPAN`] bytes.
fn called_by(core: &Image, wrapper: &CStr, target: usize, what: &str) -> Result<()> {
    let w = core.export(wrapper)?;
    ensure!(!core.calls_in(w, WRAPPER_SPAN - 1, target)?.is_empty(), "{what}: not the function {wrapper:?} calls");
    Ok(())
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let incref = core.find_unique(INCREF, "Value IncRef")?;
    let decref = core.find_unique(DECREF, "Value DecRef")?;
    let assign_fn = core.find_unique(ASSIGN, "Value assign")?;
    // They must be the reference count functions BYOND's own API uses, and assign must call the IncRef and
    // tail-jump to the DecRef just found.
    called_by(core, c"ByondValue_IncRef", incref, "Value IncRef")?;
    called_by(core, c"ByondValue_DecRef", decref, "Value DecRef")?;
    ensure!(
        core.call_target(assign_fn + 32)? == incref && core.branch_at(assign_fn + 52)?.target == decref,
        "Value assign: call/jmp targets are not IncRef/DecRef"
    );
    // eax and ebx: the high dword's register in IncRef and DecRef.
    let (inc, dec) = (refcount(0), refcount(3));
    core.untouched(&[(incref + 8, inc.len()), (decref + 8, dec.len()), (assign_fn + 7, ASSIGN_LEN)])?;
    core.write_code(incref + 8, &inc)?;
    core.write_code(decref + 8, &dec)?;
    core.write_stub(assign_fn + 7, ASSIGN_LEN, &assign(incref))?;
    Ok(format!(
        "IncRef +{:#x}, DecRef +{:#x}, assign +{:#x}",
        incref - core.base,
        decref - core.base,
        assign_fn - core.base
    ))
}
