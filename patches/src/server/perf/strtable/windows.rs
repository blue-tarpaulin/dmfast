//! `strtable`, Windows: byondcore's string load pass hooked by [`load`].

use crate::base::code::{Foreign, live_u32, trampoline, write};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};
use crate::server::perf::crc::HOOKED;
use crate::server::perf::strtree::{SITES, load_all};

/// The pass: push ebp ; mov ebp,esp ; mov ecx,[count] ; sub esp,8 ; mov byte [loading],1 ; push ebx ; xor ebx,ebx ;
/// test ecx,ecx ; je end ; push esi ; push edi ; mov eax,[table] ; loop: mov edi,[eax+ebx*4] ; ... ; call lookup ;
/// ... ; mov edx,[seed] ; ... ; call crc ; ... ; call crc ; ... ; end: mov byte [loading],0 ; call rebuild ; pop ebx ;
/// ret
const PASS: &str = "55 8B EC 8B 0D ?? ?? ?? ?? 83 EC 08 C6 05 ?? ?? ?? ?? 01 53 33 DB 85 C9 0F 84 9B 00 00 00 56 57 A1 \
                    ?? ?? ?? ?? 8B 3C 98 85 FF 0F 84 7E 00 00 00 8D 45 FC 89 5F 04 50 57 C7 47 0C 00 00 00 00 C7 47 08 \
                    00 00 00 00 E8 ?? ?? ?? ?? 83 C4 08 85 C0 75 09 8B 45 FC 85 C0 74 02 89 38 8B 77 18 C7 47 10 00 00 \
                    00 00 8B 15 ?? ?? ?? ?? 89 5D F8 85 F6 75 1A 8B 07 85 C0 74 14 8B F0 8D 4E 01 0F 1F 00 8A 06 46 84 \
                    C0 75 F9 2B F1 89 77 18 6A 04 8D 45 F8 50 52 E8 ?? ?? ?? ?? 46 56 FF 37 50 E8 ?? ?? ?? ?? 89 47 14 \
                    83 C4 18 8B 0D ?? ?? ?? ?? 43 3B D9 0F 82 69 FF FF FF 5F 5E C6 05 ?? ?? ?? ?? 00 E8 ?? ?? ?? ?? 5B \
                    8B E5 5D C3";
const COUNT: usize = 5;
const LOADING: usize = 0xE;
const TABLE: usize = 0x21;
const LOOKUP_CALL: usize = 0x46;
const SEED: usize = 0x67;
const CRC_CALLS: [usize; 2] = [0x93, 0x9D];
const LOOP_COUNT: usize = 0xAA;
const END_LOADING: usize = 0xBB;
const REBUILD_CALL: usize = 0xC0;
/// Its first instructions, moved into the trampoline: push ebp ; mov ebp,esp ; mov ecx,[count]. Not `Image::hook`:
/// `[count]` is relocated, which `untouched` refuses.
const ENTRY: usize = 9;

/// What install found.
pub(super) struct Found {
    /// The global holding the CRC seed.
    seed: usize,
    /// BYOND's pass.
    pub(super) original: Foreign<unsafe extern "C" fn()>,
    /// `DMFAST_VERIFY_STRTABLE` is set.
    pub(super) check: bool,
}

pub(super) static FOUND: Installed<Found> = Installed::new();

/// Replaces the pass (cdecl, no arguments).
extern "C" fn load() {
    let (f, loading) = (FOUND.get(), SITES.get().loading);
    // SAFETY: called by the loader in place of its pass: BYOND's globals (install) are live and the table loaded;
    // the original is the pass's trampoline.
    unsafe {
        write(loading, 1u8);
        if !load_all(live_u32(f.seed)) {
            // Not loaded (nothing was changed): BYOND's own pass.
            f.original.get()();
            return;
        }
        write(loading, 0u8);
        if f.check {
            super::verify::check(f);
        }
    }
}

/// `strtree` and `crc` are installed.
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    ensure!(!verify(), "string tree index not in fast mode");
    let s = SITES.get();
    let f = core.find_unique(PASS, "string table load")?;
    for (at, global) in
        [(COUNT, s.count), (LOOP_COUNT, s.count), (TABLE, s.table), (LOADING, s.loading), (END_LOADING, s.loading)]
    {
        ensure!(core.ptr_at(f + at)? == global, "string table load: unexpected globals");
    }
    ensure!(
        core.call_target(f + LOOKUP_CALL)? == s.lookup && core.call_target(f + REBUILD_CALL)? == s.rebuild,
        "string table load: unexpected lookup or rebuild"
    );
    // Both CRC calls go to BYOND's CRC, which `crc` hooked.
    for at in CRC_CALLS {
        ensure!(core.call_target(f + at)? == *HOOKED.get(), "string table load: unexpected CRC calls");
    }

    let original = trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?;
    let check = std::env::var_os("DMFAST_VERIFY_STRTABLE").is_some();
    let seed = core.ptr_at(f + SEED)?;
    FOUND.commit(cx, Found { seed, original: Foreign::at(original), check });
    core.jump(f, 5, load as extern "C" fn())?;
    Ok(core.report(&[f]))
}
