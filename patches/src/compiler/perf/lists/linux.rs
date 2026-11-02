//! `lists`, Linux: the intern is replaced at entry; its capacity, count, and array come from its own code, `operator
//! new` from its `push 18h ; call` site (malloc and realloc are process-wide).

use super::{FOUND, Found, intern};
use crate::base::code::{Foreign, MallocFn, ReallocFn};
use crate::base::feature::Ctx;
use crate::base::linux::alloc::{malloc, realloc};
use crate::base::{Result, bail};

/// The intern: push ebp ; push edi ; push esi ; push ebx ; sub esp,1Ch ; mov ebx,[root] ; mov esi,[esp+30h] ;
/// test ebx,ebx ; je new ; mov edi,[esi+8] ; xor ebp,ebp
const INTERN: &str = "55 57 56 53 83 EC 1C 8B 1D ?? ?? ?? ?? 8B 74 24 30 85 DB 0F 84 ?? ?? ?? ?? 8B 7E 08 31 ED";
/// Its growth: mov edx,[capacity] ; mov [edi+0Ch],esi ; cmp edx,esi ; ja append ; ... ; lea edx,[edx+ecx+10h] ;
/// mov [capacity],edx ; shl edx,2 ; push edx ; push eax ; call realloc ; add esp,10h ; mov [array],eax ;
/// test eax,eax
const GROWTH: &str = "8B 15 ?? ?? ?? ?? 89 77 0C 39 F2 77 ?? 89 F1 83 EC 08 C1 E9 04 8D 54 0A 10 89 15 ?? ?? ?? ?? \
                      C1 E2 02 52 50 E8 ?? ?? ?? ?? 83 C4 10 A3 ?? ?? ?? ?? 85 C0";
/// The capacity it compares, the capacity it stores, and the array it stores.
const CAPACITY: usize = 2;
const CAPACITY_STORE: usize = 0x1B;
const ARRAY: usize = 0x2D;
/// Its append: mov [count],esi ; mov [eax+esi*4-4],edi ; cmp esi,0FFFFh
const APPEND: &str = "89 35 ?? ?? ?? ?? 89 7C B0 FC 81 FE FF FF 00 00";
/// How far the intern reaches, for the searches in it.
const SPAN: usize = 0x240;
/// The hooked bytes.
const ENTRY: usize = 7;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(INTERN, "list intern")?;
    let grow = core.find_unique_in(f, SPAN, GROWTH, "list intern growth")?;
    let capacity = core.ptr_at(grow + CAPACITY)?;
    if core.ptr_at(grow + CAPACITY_STORE)? != capacity {
        bail!("list intern: growth does not store the capacity it compares");
    }
    let append = core.find_unique_in(f, SPAN, APPEND, "list intern append")?;
    // sub esp,0Ch ; push 18h ; call operator new
    let Some(call) = core.find_bytes_in(f, SPAN, &[0x83, 0xEC, 0x0C, 0x6A, 0x18, 0xE8])?.first() else {
        bail!("list intern: operator new call not found");
    };
    let found = Found {
        array: core.ptr_at(grow + ARRAY)?,
        count: core.ptr_at(append + 2)?,
        capacity,
        new: Foreign::at(core.rel32(call + 6)?),
        malloc: Foreign::at(malloc as MallocFn as usize),
        realloc: Foreign::at(realloc as ReallocFn as usize),
    };
    core.untouched(&[(f, ENTRY)])?;
    core.jump(f, ENTRY, intern as unsafe extern "C" fn(usize) -> u32)?;
    FOUND.commit(cx, found);
    Ok(format!("at +{:#x}", f - core.base))
}
