//! `profself`, Windows: the recursion test, error path, and entry block are contiguous; the block's join is the
//! `mov esi,[ebp+x]` after it. Frame setup's clocks are in a separate function.

use super::{is_message, move_test};
use crate::base::code::trampoline;
use crate::base::feature::Ctx;
use crate::base::{Result, bail, ensure};

/// The recursion test: add word [counter],-1 ; mov eax,[flags] ; jne block ; test al,2 ; jne block. The error path:
/// call ; test eax,eax ; jne ; push eax ; call ; add esp,4 ; push message ; call error ; add esp,4 ; jmp return path.
/// The profiler's entry block (test eax,10000h ; je join-5 ; ...), its branches inside, ending at
/// mov eax,[current frame] ; mov esi,[ebp+x] (the join).
const TEST: &str = "66 83 05 ?? ?? ?? ?? FF A1 ?? ?? ?? ?? 75 28 A8 02 75 24 E8 ?? ?? ?? ?? 85 C0 75 09 50 E8 ?? ?? \
     ?? ?? 83 C4 04 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 04 E9 ?? ?? ?? ?? A9 00 00 01 00 0F 84 ED 00 \
     00 00 E8 ?? ?? ?? ?? 8B F0 8B FA A1 ?? ?? ?? ?? 8B 48 ?? 85 C9 0F 84 B1 00 00 00 8B 09 85 C9 0F \
     84 A7 00 00 00 FF 31 E8 ?? ?? ?? ?? 83 C4 04 89 85 ?? ?? ?? ?? 85 C0 0F 84 8A 00 00 00 8B 0D ?? \
     ?? ?? ?? 8B 49 ?? FF B1 ?? ?? ?? ?? FF B1 ?? ?? ?? ?? 57 56 E8 ?? ?? ?? ?? 83 C4 10 52 50 8B 85 \
     ?? ?? ?? ?? FF 70 ?? FF 70 ?? E8 ?? ?? ?? ?? 8B 8D ?? ?? ?? ?? 83 C4 10 89 41 ?? 89 51 ?? E8 ?? \
     ?? ?? ?? 84 C0 74 40 A1 ?? ?? ?? ?? 8B 40 ?? FF B0 ?? ?? ?? ?? FF B0 ?? ?? ?? ?? 57 56 E8 ?? ?? \
     ?? ?? 83 C4 10 52 50 8B 85 ?? ?? ?? ?? FF 70 ?? FF 70 ?? E8 ?? ?? ?? ?? 8B 8D ?? ?? ?? ?? 83 C4 \
     10 89 41 ?? 89 51 ?? A1 ?? ?? ?? ?? 89 B0 ?? ?? ?? ?? 89 B8 ?? ?? ?? ?? 83 78 ?? 00 75 16 E8 ?? \
     ?? ?? ?? 8B 3D ?? ?? ?? ?? 89 47 ?? 89 57 ?? A1 ?? ?? ?? ?? 8B B5 ?? ?? ?? ??";
/// Its counter, flags, first `jne block`, error path, message push, entry block, and the block's clock call field.
const COUNTER: usize = 3;
const FLAGS: usize = 9;
const JNE: usize = 0xD;
const ERROR: usize = 0x13;
const MESSAGE_PUSH: usize = 0x26;
const BLOCK: usize = 0x37;
const CLOCK: usize = 0x43;
/// The block's store of the frame's self-time clock: mov [eax+self],esi ; mov [eax+self+4],edi (the disps).
const SELF_LO: usize = 0x10E;
const SELF_HI: usize = 0x114;
/// Frame setup for a new frame with profiling on: test [flags],10000h ; je done ; count the call ; call real clock ;
/// mov ecx,[frame] ; store the real start ; call clock ; mov esi,[frame] ; store the total start
/// (mov [esi+t],eax ; mov [esi+t+4],edx, t a disp8 or disp32) ; done:
const SETUP: &str = "F7 05 ?? ?? ?? ?? 00 00 01 00 74 ?? 8B 01 FF 30 E8 ?? ?? ?? ?? 83 C4 04 85 C0 74 02 FF 00 E8 ?? ?? \
                     ?? ?? 8B 0D ?? ?? ?? ?? 89 41 ?? 89 51 ?? E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 89";
/// Its flags, `je done` field, clock call field, and total-time store.
const SETUP_FLAGS: usize = 2;
const DONE_JE: usize = 0xB;
const SETUP_CLOCK: usize = 0x30;
const STORE: usize = 0x3A;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(TEST, "recursion test and profiler entry")?;
    ensure!(is_message(core, core.ptr_at(at + MESSAGE_PUSH)?)?, "recursion test: not the recursion error");
    // je join-5
    let join = core.rel32(at + BLOCK + 7)? + 5;
    let flags = core.u32_at(at + FLAGS)?;
    let clock = core.rel32(at + CLOCK)?;
    let self_clock = core.u32_at(at + SELF_LO)?;
    ensure!(
        core.u32_at(at + SELF_HI)? == self_clock.wrapping_add(4),
        "profiler entry: self-time clock not one 8-byte field"
    );

    let setup = core.find_unique(SETUP, "frame setup profiler clocks")?;
    let store = setup + STORE;
    let store_len = match core.bytes(store, 9)? {
        [0x89, 0x86, _, _, _, _, 0x89, 0x96, _] => 12,
        [0x89, 0x46, _, 0x89, 0x96, _, _, _, _] => 9,
        _ => bail!("frame setup: total-time clock store not as expected"),
    };
    ensure!(
        core.u32_at(setup + SETUP_FLAGS)? == flags
            && core.rel32(setup + SETUP_CLOCK)? == clock
            && core.rel8(setup + DONE_JE)? == store + store_len,
        "frame setup: not the profiler's flag, clock or layout"
    );

    // Frame setup also starts the self-time clock: the total-time stores (moved) ; mov [esi+self],eax ;
    // mov [esi+self+4],edx ; jmp back
    let mut start = core.bytes(store, store_len)?.to_vec();
    start.extend([0x89, 0x86]);
    start.extend(self_clock.to_le_bytes());
    start.extend([0x89, 0x96]);
    start.extend(self_clock.wrapping_add(4).to_le_bytes());
    core.jump(store, store_len, trampoline(&start, store + store_len)?)?;
    move_test(core, at + JNE, join, core.u32_at(at + COUNTER)?, flags, at + ERROR)?;
    Ok(format!("at +{:#x}, join at +{:#x}, frame setup at +{:#x}", at - core.base, join - core.base, store - core.base))
}
