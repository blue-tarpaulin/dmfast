//! `profcpu`, Linux: the one `clock()` call, in `TimeLib::CpuClock`, calls [`thread_clock`] instead.
//!
//! BYOND reads the clock three times per proc call while profiling, and the thread's CPU time costs a system call
//! (~0.5 µs). The reads come in tight bursts, so within [`WINDOW_NS`] of a reading from the kernel, the clock is that
//! reading plus the wall time since (`CLOCK_MONOTONIC`, read without a system call). Wall time runs ahead of CPU
//! time only while the thread is switched out, so a reading is off by at most [`WINDOW_NS`], and only when that
//! happens inside a window. The values a thread gets never go back.

use std::cell::Cell;

use crate::base::feature::Ctx;
use crate::base::{Result, bail, ensure};

type ClockFn = unsafe extern "C" fn() -> libc::clock_t;

unsafe extern "C" {
    /// The C library's `clock()` (the libc crate doesn't declare it).
    #[link_name = "clock"]
    fn libc_clock() -> libc::clock_t;
}

/// How long after a kernel reading the clock counts on from it.
const WINDOW_NS: u64 = 5_000;

/// A thread's clock: its last kernel reading and the wall time then, and the last value returned, in ns.
struct Clock {
    kernel: Cell<Option<(u64, u64)>>,
    last: Cell<u64>,
}

thread_local! {
    static CLOCK: Clock = const { Clock { kernel: Cell::new(None), last: Cell::new(0) } };
}

/// `clock` now in ns, or `None` if the kernel refuses.
fn read(clock: libc::clockid_t) -> Option<u64> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out pointer.
    (unsafe { libc::clock_gettime(clock, &mut ts) } == 0).then(|| ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64)
}

/// `clock()` for the calling thread: its CPU time in microseconds, truncated to `clock_t` as glibc's is.
unsafe extern "C" fn thread_clock() -> libc::clock_t {
    let Some(wall) = read(libc::CLOCK_MONOTONIC) else { return -1 };
    let ns = CLOCK.with(|c| {
        let now = match c.kernel.get() {
            Some((cpu, at)) if wall - at < WINDOW_NS => cpu + (wall - at),
            _ => {
                let cpu = read(libc::CLOCK_THREAD_CPUTIME_ID)?;
                c.kernel.set(Some((cpu, wall)));
                cpu
            }
        };
        let now = now.max(c.last.get());
        c.last.set(now);
        Some(now)
    });
    ns.map_or(-1, |ns| (ns / 1000) as libc::clock_t)
}

/// `TimeLib::CpuClock`: push ebx ; sub esp,8 ; mov ebx,[esp+10h] ; mov dword [ebx],0 ; call clock ; mov [ebx+4],eax ;
/// add esp,8 ; mov eax,ebx ; pop ebx ; ret 4
const CPU_CLOCK: &str = "53 83 EC 08 8B 5C 24 10 C7 03 00 00 00 00 E8 ?? ?? ?? ?? 89 43 04 83 C4 08 89 D8 5B C2 04 00";
/// Its `call clock`.
const CALL: usize = 14;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.import_calls("clock", libc_clock as ClockFn)?;
    let [site] = sites[..] else { bail!("clock(): expected 1 call, found {}", sites.len()) };
    let cpu_clock = core.export(c"_ZN7TimeLib8CpuClockEv")?;
    ensure!(site == cpu_clock + CALL && core.is_at(cpu_clock, CPU_CLOCK)?, "TimeLib::CpuClock: not as expected");
    core.call(site, 5, thread_clock as ClockFn)?;
    Ok(core.report(&[site]))
}

#[cfg(test)]
mod tests {
    use super::{read, thread_clock};

    /// Over ~20 ms of work read in tight bursts, the clock advances as the kernel's thread CPU clock does.
    #[test]
    fn follows_the_thread_cpu_clock() {
        let kernel = || read(libc::CLOCK_THREAD_CPUTIME_ID).unwrap();
        // SAFETY: no arguments; reads this thread's clock.
        let (c0, k0) = (unsafe { thread_clock() }, kernel());
        let mut x = 0u64;
        while kernel() - k0 < 20_000_000 {
            for i in 0..1000 {
                x = x.wrapping_add(std::hint::black_box(i));
            }
            // SAFETY: as above.
            std::hint::black_box(unsafe { thread_clock() });
        }
        std::hint::black_box(x);
        // SAFETY: as above.
        let (c1, k1) = (unsafe { thread_clock() }, kernel());
        let (clock_us, kernel_us) = (c1.wrapping_sub(c0) as i64, ((k1 - k0) / 1000) as i64);
        assert!((clock_us - kernel_us).abs() < 1000, "clock {clock_us} us, kernel {kernel_us} us");
    }
}
