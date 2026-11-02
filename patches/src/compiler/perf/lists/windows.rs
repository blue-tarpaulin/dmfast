//! `lists`, Windows: the intern is replaced at entry; its array, count, and capacity are the globals right before the
//! tree's root.

use super::{FOUND, Found, intern};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The intern: push ebp ; mov ebp,esp ; sub esp,0Ch ; push ebx ; push esi ; push edi ; mov edi,[root] ;
/// xor edx,edx ; mov [ebp-4],edx ; mov esi,offset root ; test edi,edi ; je ... ; mov esi,[ebp+8] ;
/// mov eax,[esi+8]
const INTERN: &str =
    "55 8B EC 83 EC 0C 53 56 57 8B 3D ?? ?? ?? ?? 33 D2 89 55 FC BE ?? ?? ?? ?? 85 FF 74 ?? 8B 75 08 8B 46 08";
/// The two references to the root.
const ROOT: [usize; 2] = [11, 21];
/// How far the intern reaches, for the searches in it.
const SPAN: usize = 0x240;
/// The hooked bytes.
const ENTRY: usize = 5;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(INTERN, "list intern")?;
    let root = core.ptr_at(f + ROOT[0])?;
    if core.ptr_at(f + ROOT[1])? != root {
        bail!("list intern: root mismatch");
    }
    // The entry array, count, and capacity sit right before the root; the intern must use all three.
    let (array, count, capacity) = (root - 0xC, root - 8, root - 4);
    for a in [array, count, capacity] {
        if !core.mentions(f, SPAN, a)? {
            bail!("list intern: entry array/count/capacity not where expected");
        }
    }
    // push 18h ; call operator new
    let Some(call) = core.find_bytes_in(f, SPAN, &[0x6A, 0x18, 0xE8])?.first() else {
        bail!("list intern: operator new call not found");
    };
    let (malloc, realloc) = core.crt_alloc()?;
    let found = Found {
        array,
        count,
        capacity,
        new: Foreign::at(core.rel32(call + 3)?),
        malloc: Foreign::at(malloc),
        realloc: Foreign::at(realloc),
    };
    core.untouched(&[(f, ENTRY)])?;
    core.jump(f, ENTRY, intern as unsafe extern "C" fn(usize) -> u32)?;
    FOUND.commit(cx, found);
    Ok(format!("at +{:#x}", f - core.base))
}
