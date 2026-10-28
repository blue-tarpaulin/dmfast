//! `pickweight`, Windows: a wrapper runs the stock opcode and redirects a jump to a threshold.

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::base::code::{Foreign, live_u32, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

static ORIGINAL: Foreign<unsafe extern "C" fn()> = Foreign::new();
/// The global holding the interpreter state: `+0x10` the code (one dword per word), `+0x14` the program counter.
static STATE: AtomicUsize = AtomicUsize::new(0);

extern "C" fn pick() {
    // SAFETY: STATE is BYOND's interpreter state global and ORIGINAL the relocated opcode start, set at install.
    unsafe {
        let state = || live_u32(STATE.load(Relaxed)) as usize;
        let code = live_u32(state() + 0x10) as usize;
        let word = |i: usize| live_u32(code + 4 * i) as u16;
        let start = usize::from(live_u32(state() + 0x14) as u16);
        ORIGINAL.get()();
        let pc = (state() + 0x14) as *mut u16;
        let last = start + 2 + 2 * usize::from(word(start + 1));
        let target = word(last);
        // A jump stores target - 1.
        if target >= super::THRESHOLD && *pc == target.wrapping_sub(1) {
            *pc = word(last + 1).wrapping_sub(1);
        }
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // push ebp ; mov ebp,esp ; mov edx,[state] ; sub esp,8 ; inc word [edx+14h] ; cmp [rand_mode],1F4h ;
    // movzx ecx,word [edx+14h] ; mov eax,[edx+10h] ; push ebx ; push esi ; push edi ; movzx esi,word [eax+ecx*4]
    let f = core.find_unique(
        "55 8B EC 8B 15 ?? ?? ?? ?? 83 EC 08 66 FF 42 14 81 3D ?? ?? ?? ?? F4 01 00 00 0F B7 4A 14 8B 42 10 \
         53 56 57 0F B7 34 88",
        "weighted pick opcode",
    )?;
    // Pins the operand layout: per option, inc word [pc] (the threshold) ; ... ; movzx eax,word [code+pc*4] ;
    // inc ecx ; mov [pc],cx (the target) ; cmp bx,ax ; jbe picked ; then the final target: inc word [pc] ; ... ;
    // movzx eax,word [eax+ecx*4] ; dec ax ; mov [pc],ax.
    let tail = core.find_unique(
        "66 FF 42 14 81 C6 FF FF 00 00 0F B7 4A 14 0F B7 04 8F 41 66 89 4A 14 66 3B D8 76 ?? 66 85 F6 75 ?? \
         66 FF 42 14 0F B7 4A 14 8B 42 10 5F 5E 5B 0F B7 04 88 66 48 66 89 42 14",
        "weighted pick final target",
    )?;
    if !(f + 1..=f + 0x100).contains(&tail) {
        bail!("weighted pick final target: not in the opcode found");
    }
    STATE.store(core.ptr_at(f + 5)?, Relaxed);
    ORIGINAL.set(trampoline(core.bytes(f, 12)?, f + 12)?);
    core.jump(f, 12, pick as extern "C" fn())?;
    Ok(format!("at {:#x}", f - core.base))
}
