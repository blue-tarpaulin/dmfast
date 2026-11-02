//! `tickinit` (forum reports 2968161, 2968160, and 2968159) and `tick0` (forum report 2968158): the world's tick
//! bookkeeping at start and reboot.
//!
//! The world reset (at start and every reboot) zeroes the clock, `world.tick`, and the tick counter, but not the tick
//! start time or the `world.cpu` and `world.map_cpu` averages. `tickinit` hooks the reset to clear the averages and
//! start the tick clock, so `tick_usage` before the first tick is valid, and pushes the start-up usage into the
//! average on the first tick. `tick0` makes the first tick call `/world/Tick` once more, at `world.time` 0.
//!
//! Both share one hook, which replaces the tick timer's registered callback rather than the tick's first bytes:
//! profilers such as byond-tracy detour the tick's entry by copying its first instructions, and a jump of ours there
//! would be copied unrelocated. The hook does its work only on a tick after a reset, as the tick itself would, then
//! runs the stock tick.

use crate::base::code::{Foreign, hx, live_u32, read, trampoline, write};
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::image::{Hits, Image};
use crate::base::sync::Installed;
use crate::base::{Result, bail, ensure};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

pub static TICKINIT: Feature = Feature::fix(
    Hosts::SERVER,
    "tickinit",
    "tick_usage/world.cpu start fresh at start and reboot",
    Tier::Extended,
    install!(windows: install_init, linux: install_init),
);

pub static TICK0: Feature = Feature::fix(
    Hosts::SERVER,
    "tick0",
    "/world/Tick also runs at world.time 0",
    Tier::Extended,
    install!(windows: install_tick0, linux: install_tick0),
);

type TicksFn = unsafe extern "C" fn() -> u64;
/// The tick (a timer callback; its arguments, if any, are passed through).
type TickFn = unsafe extern "C" fn(usize, usize, usize) -> i32;

/// A running average of 16 samples (f32): its index byte, samples, sum, and scale.
struct Avg {
    index: usize,
    samples: usize,
    sum: usize,
    scale: usize,
}

/// What the hooks need, found in the tick and checked before anything is hooked.
struct Tick {
    /// The timer's original callback: the tick or a thunk to it.
    original: Foreign<TickFn>,
    /// `TimeLib::Ticks()`.
    ticks: Foreign<TicksFn>,
    /// The tick start time (u64).
    start: usize,
    /// `world.tick`: ticks since the reset.
    tick_count: usize,
    /// The world pointer: the tick does nothing while it is null.
    world: usize,
    /// The flag byte set while a tick runs.
    in_tick: usize,
    /// The `.dmb` version: `/world/Tick` is only called from 0x203 on.
    version: usize,
    /// The tick usage and the call of `/world/Tick`, as the tick makes them.
    calls: os::Calls,
    /// `world.cpu`'s average, its result, and the result without the map's share.
    cpu: Avg,
    cpu_out: usize,
    cpu_net: usize,
    /// `world.map_cpu`'s average and its result.
    map: Avg,
    map_out: usize,
    /// Counts ticks between `world.map_cpu` measurements (u16).
    map_skip: usize,
}

static TICK: Installed<Tick> = Installed::new();
/// The stock world reset.
static RESET: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// `tickinit`: hooks the world reset and the tick.
fn install_init(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let reset = core.find_unique(os::RESET, "world reset")?;
    // 16-byte aligned, or after padding or a return.
    let entry = reset.is_multiple_of(16) || matches!(core.u8_at(reset - 1)?, 0xCC | 0xC3 | 0x90);
    ensure!(entry, "the world reset match is not a function entry");
    RESET.set(trampoline(core.bytes(reset, os::RESET_ENTRY)?, reset + os::RESET_ENTRY)?);
    core.jump(reset, os::RESET_ENTRY, reset_entry as unsafe extern "C" fn())?;
    hook_tick(cx)?;
    Ok(format!("reset {}", core.report(&[reset])))
}

fn install_tick0(cx: &Ctx) -> Result<String> {
    hook_tick(cx)?;
    Ok("tick hook".into())
}

/// Replaces the tick timer's callback with [`tick_entry`], once for both fixes.
fn hook_tick(cx: &Ctx) -> Result<()> {
    if TICK.is_set() {
        return Ok(());
    }
    let core = cx.core();
    let (tick, site) = os::locate(core)?;
    core.write_code(site, &(tick_entry as TickFn as usize as u32).to_le_bytes())?;
    TICK.commit(cx, tick);
    Ok(())
}

/// The operand of the timer registration's `push imm32` of the tick `f` (or of a `jmp rel32` thunk to it).
fn registration(core: &Image, f: usize) -> Result<usize> {
    let code = core.code_ranges();
    // The rel32 or imm32 of the 5-byte instruction `op` at `at`, if it is one in code.
    let operand = |at: usize, op: u8| {
        if !code.iter().any(|&(s, len)| at >= s && at + 5 <= s + len) {
            return None;
        }
        let c = core.haystack(at, 5).ok()?;
        (c[0] == op).then(|| u32::from_le_bytes([c[1], c[2], c[3], c[4]]) as usize)
    };
    let thunk = |v: usize| operand(v, 0xE9).is_some_and(|rel| (v + 5).wrapping_add(rel) == f);
    let sites: Hits =
        core.reloc_sites().into_iter().filter(|&s| operand(s - 1, 0x68).is_some_and(|v| v == f || thunk(v))).collect();
    let site = sites.unique("tick timer registration")?;
    core.rely(site - 1, 5)?;
    let v = core.ptr_at(site)?;
    if v != f {
        core.rely(v, 5)?;
    }
    Ok(site)
}

/// The in-tick flag: the first flag the tick `f` sets to 1 (mov byte [flag],1) and clears once, within `span`.
fn in_tick_flag(core: &Image, f: usize, span: usize) -> Result<usize> {
    for h in core.find_in(f, span, "C6 05 ?? ?? ?? ?? 01")? {
        let flag = core.ptr_at(h + 2)?;
        if core.find_in(f, span, &format!("C6 05 {} 00", hx(flag))).is_ok_and(|v| v.len() == 1) {
            return Ok(flag);
        }
    }
    bail!("in-tick flag not found")
}

/// The tick timer's callback, replaced: on the first tick after a reset, runs what the installed fixes ask for first.
///
/// # Safety
/// Only as the tick timer's callback, which [`hook_tick`] commits with the state.
unsafe extern "C" fn tick_entry(a: usize, b: usize, c: usize) -> i32 {
    let t = TICK.get();
    // SAFETY: the state's globals and functions were located and checked at install.
    unsafe {
        if live_u32(t.world) != 0 && live_u32(t.tick_count) == 0 {
            if TICKINIT.applied() {
                sample_start(t);
            }
            if TICK0.applied() {
                call_world_tick(t);
            }
        }
        t.original.get()(a, b, c)
    }
}

/// Pushes the start-up's usage into the `world.cpu` average, as a finished tick does.
///
/// # Safety
/// From the tick timer's callback.
unsafe fn sample_start(t: &Tick) {
    // SAFETY: the average's globals were located at install.
    unsafe {
        if read::<u64>(t.start) == 0 {
            return;
        }
        let usage = t.calls.usage(t);
        let idx = usize::from(read::<u8>(t.cpu.index));
        let slot = t.cpu.samples + idx * 4;
        let sum = read::<f32>(t.cpu.sum) - read::<f32>(slot) + usage;
        write(slot, usage);
        write(t.cpu.sum, sum);
        write(t.cpu.index, ((idx + 1) & 0xF) as u8);
        let out = sum * read::<f32>(t.cpu.scale);
        write(t.cpu_out, out);
        write(t.cpu_net, out - read::<f32>(t.map_out));
    }
}

/// Calls `/world/Tick` as the tick does, if the `.dmb` is new enough.
///
/// # Safety
/// From the tick timer's callback.
unsafe fn call_world_tick(t: &Tick) {
    // SAFETY: the globals and functions were located at install; called from the tick, as BYOND does.
    unsafe {
        if live_u32(t.version) < 0x203 {
            return;
        }
        // Start the tick clock as a tick does, so `tick_usage` in this call is its own. The tick resets it.
        write(t.start, t.ticks.get()());
        let was = read::<u8>(t.in_tick);
        write(t.in_tick, 1u8);
        t.calls.world_tick();
        write(t.in_tick, was);
    }
}

/// The world reset, hooked: after it, the tick clock restarts and the usage averages are empty.
///
/// # Safety
/// Only as the world reset's replacement.
unsafe extern "C" fn reset_entry() {
    // SAFETY: the stock reset's trampoline; the state's globals were located at install.
    unsafe {
        RESET.get()();
        if !TICK.is_set() {
            return;
        }
        let t = TICK.get();
        write(t.start, t.ticks.get()());
        for a in [&t.cpu, &t.map] {
            write(a.samples, [0f32; 16]);
            write(a.sum, 0f32);
            write(a.index, 0u8);
        }
        for out in [t.cpu_out, t.cpu_net, t.map_out] {
            write(out, 0f32);
        }
        write(t.map_skip, 0u16);
    }
}
