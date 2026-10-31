//! `profcpu`, Linux: the one `clock()` call, in `TimeLib::CpuClock`, calls [`thread_clock`] instead.

use crate::base::feature::Ctx;
use crate::base::{Result, bail};

type ClockFn = unsafe extern "C" fn() -> libc::clock_t;

unsafe extern "C" {
    /// The C library's `clock()` (the libc crate doesn't declare it).
    #[link_name = "clock"]
    fn libc_clock() -> libc::clock_t;
}

/// `clock()` for the calling thread: its CPU time in microseconds, truncated to `clock_t` as glibc's is.
unsafe extern "C" fn thread_clock() -> libc::clock_t {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out pointer.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
        return -1;
    }
    (ts.tv_sec as i64 * 1_000_000 + ts.tv_nsec as i64 / 1000) as libc::clock_t
}

/// `TimeLib::CpuClock`: push ebx ; sub esp,8 ; mov ebx,[esp+10h] ; mov dword [ebx],0 ; call clock ; mov [ebx+4],eax ;
/// add esp,8 ; mov eax,ebx ; pop ebx ; ret 4
const CPU_CLOCK: &str = "53 83 EC 08 8B 5C 24 10 C7 03 00 00 00 00 E8 ?? ?? ?? ?? 89 43 04 83 C4 08 89 D8 5B C2 04 00";
/// Its `call clock`.
const CALL: usize = 14;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.import_calls("clock", libc_clock as ClockFn as usize)?;
    let [site] = sites[..] else { bail!("clock(): expected 1 call, found {}", sites.len()) };
    let cpu_clock = core.export(c"_ZN7TimeLib8CpuClockEv")?;
    if site != cpu_clock + CALL || !core.is_at(cpu_clock, CPU_CLOCK)? {
        bail!("TimeLib::CpuClock: not as expected");
    }
    core.call(site, 5, thread_clock as ClockFn)?;
    Ok(format!("at +{:#x}", site - core.base))
}
