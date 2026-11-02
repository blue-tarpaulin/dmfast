//! `profself`, Linux: the recursion test `je`s out of line to the error path and `jne`s to the entry block, which
//! jumps back to the join. 515.1647's join differs, so the fix doesn't install there.

use super::{MESSAGE, is_message, move_test};
use crate::base::code::{hx, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The recursion test: sub word [counter],1 ; mov eax,[flags] ; jne ok ; test al,2 ; je error ;
/// ok: mov esi,[current frame] ; test eax,10000h ; jne block ; join: mov eax,[ebp+x]
const TEST: &str = "66 83 2D ?? ?? ?? ?? 01 A1 ?? ?? ?? ?? 75 08 A8 02 0F 84 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? A9 00 00 01 \
                    00 0F 85 ?? ?? ?? ?? 8B 85 ?? ?? ?? ??";
/// Its counter, flags, `jne ok`, `je error` field, `jne block` field, and the join.
const COUNTER: usize = 3;
const FLAGS: usize = 9;
const JNE: usize = 0xD;
const ERROR_JE: usize = 0x13;
const BLOCK_JNE: usize = 0x24;
const JOIN: usize = 0x28;
/// The end of the error path: sub esp,0Ch ; push message ; call error ; mov esi,[current frame] ; add esp,10h ;
/// jmp return path
const ERROR_TAIL: &str = "83 EC 0C 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 10 E9";
/// The profiler's entry block: charge the caller's self time (and overtime), start the new frame's clocks. Every
/// branch stays inside except the two back to the join ([`EXITS`]).
const BLOCK: &str = "8D BD ?? ?? ?? ?? 83 EC 0C 57 E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 83 C4 0C \
    8B 46 ?? 0F 29 85 ?? ?? ?? ?? 85 C0 0F 84 D9 00 00 00 8B 00 85 C0 0F 84 CF 00 00 00 83 EC 0C FF \
    30 E8 ?? ?? ?? ?? 89 C3 83 C4 10 85 C0 0F 84 B2 00 00 00 A1 ?? ?? ?? ?? 83 EC 0C 8B 40 ?? FF B0 \
    ?? ?? ?? ?? FF B0 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? 83 C4 10 FF \
    B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? FF 73 ?? FF 73 ?? 57 E8 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 83 \
    C4 1C 66 0F D6 43 ?? E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 84 C0 74 55 8B 46 ?? 83 EC 0C FF B0 ?? ?? \
    ?? ?? FF B0 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? 83 C4 10 FF B5 ?? \
    ?? ?? ?? FF B5 ?? ?? ?? ?? FF 73 ?? FF 73 ?? 57 E8 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 83 C4 1C \
    66 0F D6 43 ?? 8B 35 ?? ?? ?? ?? F3 0F 7E ?? ?? ?? ?? ?? 8B 46 ?? 66 0F D6 ?? ?? ?? ?? ?? 85 C0 \
    0F 85 ?? ?? ?? ?? 83 EC 0C 57 E8 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 0C \
    66 0F D6 46 ?? E9 ?? ?? ?? ??";
/// The block's exits (rel32 fields): `jne join` (the real-time clock is set) and the final `jmp join`.
const EXITS: [usize; 2] = [290, 326];
/// The block's clock call field, and its store of the frame's self-time clock (`movq [esi+self],xmm`: ModRM, disp).
const CLOCK: usize = 11;
const SELF_MODRM: usize = 281;
const SELF_DISP: usize = 282;
/// Frame setup for a new frame with profiling on: count the call ; call Now ; store the real start ;
/// mov ebx,[frame] ; call CpuClock ; movq xmm0,[ebp+x] ; add esp,0Ch ; movq [ebx+total],xmm0 ; jmp done
const SETUP: &str = "8B 06 83 EC 0C FF 30 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 03 83 00 01 8D BD ?? ?? ?? ?? 83 EC 0C 8B \
                     1D ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 66 0F D6 43 ?? 8B 1D ?? ?? ?? ?? 57 E8 \
                     ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 83 C4 0C 66 0F D6 43 ?? E9 ?? ?? ?? ??";
/// Its CpuClock call field, the total-time store (`movq [ebx+total],xmm0`), and the `jmp done` field.
const SETUP_CLOCK: usize = 64;
const STORE: usize = 79;
const STORE_LEN: usize = 5;
const DONE_JMP: usize = 85;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(TEST, "recursion test")?;
    let error = core.rel32(at + ERROR_JE)?;
    let block = core.rel32(at + BLOCK_JNE)?;
    let join = at + JOIN;
    let Some(tail) = core.find_in(error, 0x80, ERROR_TAIL)?.first() else {
        bail!("recursion test: the error path doesn't raise \"{}\"", String::from_utf8_lossy(MESSAGE));
    };
    if !is_message(core, core.ptr_at(tail + 4)?)? {
        bail!("recursion test: the error path doesn't raise \"{}\"", String::from_utf8_lossy(MESSAGE));
    }
    if !core.is_at(block, BLOCK)? {
        bail!("profiler entry block: not as expected");
    }
    if !EXITS.iter().all(|&rel| core.rel32(block + rel).is_ok_and(|t| t == join)) {
        bail!("profiler entry block: does not return to the join");
    }
    let clock = core.rel32(block + CLOCK)?;
    let modrm = core.u8_at(block + SELF_MODRM)?;
    if modrm & 0xC7 != 0x86 {
        bail!("profiler entry block: self-time clock store not as expected");
    }
    let self_clock = core.u32_at(block + SELF_DISP)?;

    let setup = core.find_unique(SETUP, "frame setup profiler clocks")?;
    if core.rel32(setup + SETUP_CLOCK)? != clock {
        bail!("frame setup: not the profiler's clock");
    }
    // Reached by `test byte [flags+2],1 (profiling) ; jne setup`: the first such test is relied on, the flag's other
    // tests are candidates.
    let flags = core.u32_at(at + FLAGS)?;
    let test = format!("F6 05 {} 01 0F 85", hx(flags as usize + 2));
    let reaches = |t: &usize| core.rel32(t + 9).is_ok_and(|s| s == setup);
    let Some(t) = core.probe_all(&test)?.iter().copied().find(reaches) else {
        bail!("frame setup: not reached by the profiling test");
    };
    core.rely(t, test.split_whitespace().count())?;

    // Frame setup also starts the self-time clock: movq [ebx+total],xmm0 (moved) ; movq [ebx+self],xmm0 ; jmp done
    let store = setup + STORE;
    let mut start = core.bytes(store, STORE_LEN)?.to_vec();
    start.extend([0x66, 0x0F, 0xD6, 0x83]);
    start.extend(self_clock.to_le_bytes());
    core.jump(store, STORE_LEN + 5, trampoline(&start, core.rel32(setup + DONE_JMP)?)?)?;
    move_test(core, at + JNE, join, core.u32_at(at + COUNTER)?, flags, error)?;
    Ok(format!("at +{:#x}, join at +{:#x}, frame setup at +{:#x}", at - core.base, join - core.base, store - core.base))
}
