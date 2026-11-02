//! `initvar`, Windows: a Rust hook on `initial(tag, id, var) -> value` (cdecl, the result in EDX:EAX).

use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, bail};
use crate::server::fixes::minmax::{Tags, value_tags};

/// `initial()` and the var read: `(tag, id, var) -> value`.
type VarFn = unsafe extern "C" fn(u32, u32, u32) -> u64;

struct Found {
    tags: Tags,
    original: Foreign<VarFn>,
    read: Foreign<VarFn>,
}

static FOUND: Installed<Found> = Installed::new();

// The var read raises its error as a C++ exception through this frame: fine on x86 Windows, where SEH unwinds via the
// fs:[0] chain and this frame (panic=abort, nothing to drop) registers none.
extern "C" fn initial(tag: u32, id: u32, var: u32) -> u64 {
    let found = FOUND.get();
    let t = tag as u8;
    let f = if t == found.tags.text || t == found.tags.number { &found.read } else { &found.original };
    // SAFETY: both are cdecl (tag, id, var) -> value, found and checked at install; the arguments are BYOND's.
    unsafe { f.get()(tag, id, var) }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // push ebp ; mov ebp,esp ; mov ecx,[ebp+8] ; movzx eax,cl ; add eax,-2Ch ; cmp eax,26h ; ja default ;
    // movzx eax,[table+eax] ; jmp [table2+eax*4] ; (default:) push [ebp+0Ch] ; push ecx ; call 1020ab40
    let f = core.find_unique(
        "55 8B EC 8B 4D 08 0F B6 C1 83 C0 D4 83 F8 26 77 ?? 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ?? FF 75 0C 51 E8",
        "initial() entry",
    )?;
    // The ordinary read of var `var` on value (tag, id): an SEH-framed switch on the tag.
    let read = core.find_unique(
        "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 0C 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
         64 A3 00 00 00 00 8B 5D 08 8B 75 10 8B 7D 0C 0F B6 C3 48 83 F8 57 0F 87",
        "var read",
    )?;
    // cdecl with three arguments: some call to it is followed by `add esp,0Ch` (that call is relied on, the others
    // are not).
    let popped = |c: usize| core.bytes(c + 5, 3).is_ok_and(|b| b == [0x83, 0xC4, 0x0C]);
    let Some(c) = core.probe_calls_to(read)?.into_iter().find(|&c| popped(c)) else {
        bail!("var read is not a 3-argument cdecl function");
    };
    core.rely(c, 5)?;
    let tags = value_tags(core)?;
    // push ebp ; mov ebp,esp ; mov ecx,[ebp+8]: 6 position-independent bytes.
    let original = core.hook(f, 6, initial as extern "C" fn(u32, u32, u32) -> u64)?;
    FOUND.commit(cx, Found { tags, original: Foreign::at(original), read: Foreign::at(read) });
    Ok(format!("{}, var read {}", core.report(&[f]), core.report(&[read])))
}
