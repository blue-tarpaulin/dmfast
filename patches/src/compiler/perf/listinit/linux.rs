//! `listinit`, Linux: libbyond's list initializer intern jumps to the replacement; [`original`] runs the stock one.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use super::{Found, init, intern};
use crate::base::code::{Foreign, MallocFn, ReallocFn};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The stock intern after its first 7 bytes.
static ORIG_CONT: AtomicUsize = AtomicUsize::new(0);

/// The stock intern: its first 7 bytes (displaced by the jump), then the rest.
///
/// # Safety
/// As the stock intern: `_list` is 0 or the compiler's initializer.
#[unsafe(naked)]
unsafe extern "C" fn original(_list: usize) -> u32 {
    naked_asm!(
        "push ebp",
        "push edi",
        "push esi",
        "push ebx",
        "sub esp, 0x1c",
        "jmp dword ptr [{cont}]",
        cont = sym ORIG_CONT,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // push ebp/edi/esi/ebx ; sub esp,1Ch ; mov edi,[array] ; mov esi,[count] ; mov eax,[esp+30h] ;
    // test eax,eax ; je append ; mov eax,[esp+30h] ; movzx ebp,word [eax+4]
    let f = core.find_unique(
        "55 57 56 53 83 EC 1C 8B 3D ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 8B 44 24 30 85 C0 0F 84 ?? ?? ?? ?? \
         8B 44 24 30 0F B7 68 04",
        "list initializer intern",
    )?;
    let array = core.ptr_at(f + 9)?;
    let count = core.ptr_at(f + 15)?;
    if array != count + 4 {
        bail!("list initializer intern: entry array not right after the count");
    }
    // The append must store the new count: mov [count],eax
    let store = [&[0xA3][..], &(count as u32).to_le_bytes()].concat();
    if core.find_bytes_in(f, 0x180, &store)?.is_empty() {
        bail!("list initializer intern: count store not found");
    }
    ORIG_CONT.store(f + 7, Relaxed);
    init(Found {
        array,
        count,
        malloc: Foreign::at(crate::base::linux::alloc::malloc as MallocFn as usize),
        realloc: Foreign::at(crate::base::linux::alloc::realloc as ReallocFn as usize),
        original: Foreign::at(original as unsafe extern "C" fn(usize) -> u32 as usize),
    });
    core.jump(f, 7, intern as extern "C" fn(usize) -> u32)?;
    Ok(format!("at +{:#x}", f - core.base))
}
