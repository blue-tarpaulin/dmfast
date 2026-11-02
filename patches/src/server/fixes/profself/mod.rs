//! `profself` (forum report 2913091): in the profiler, a proc that errors on entry no longer gets a huge (or negative)
//! self CPU time.
//!
//! The recursion error (and errors in frame setup) leave before the profiler's entry block, which charges the caller
//! and starts the self-time clock. The fix moves the recursion test after the entry block: the test's `jne` becomes a
//! `jmp`, and a stub at the block's join repeats it. Frame setup starts the self-time clock with the total-time one.

use crate::base::Result;
use crate::base::code::{Cond, Stub};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::image::Image;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static PROFSELF: Feature = Feature::fix(
    Hosts::SERVER,
    "profself",
    "correct profiler self time for a proc that errors on entry (recursion limit, bad arguments)",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// The start of the recursion error's message.
const MESSAGE: &[u8] = b"Maximum recursion level reached";

/// The join's first instruction, `mov reg,[ebp+disp32]`, moved into the stub.
const JOIN_LEN: usize = 6;

/// Whether the C string at `addr` starts with [`MESSAGE`].
fn is_message(core: &Image, addr: usize) -> Result<bool> {
    Ok(core.bytes(addr, MESSAGE.len())? == MESSAGE)
}

/// Move the recursion test after the profiler's entry block: the `join` after the block goes through a stub that
/// repeats the test (the word counter at `counter`, world flags bit 1 at `flags`: `loop_checks` off) and jumps to
/// `error` when it fails, and the test's short `jne` at `jne` becomes a `jmp`.
fn move_test(core: &Image, jne: usize, join: usize, counter: u32, flags: u32, error: usize) -> Result<()> {
    let mut s = Stub::new();
    let cont = s.label();
    let stub = s
        .bytes_u32(&[0x66, 0x83, 0x3D], counter, &[0x00]) // cmp word [counter],0
        .jcc8(Cond::NotEqual, cont) // jne cont
        .bytes_u32(&[0xF6, 0x05], flags, &[0x02]) // test byte [flags],2
        .jcc8(Cond::NotEqual, cont) // jne cont
        .jmp(error) // jmp error
        .bind(cont)
        .bytes(core.bytes(join, JOIN_LEN)?) // cont: the join's first instruction
        .jmp(join + JOIN_LEN) // jmp back
        .exec()?;
    // The join first: until the test's jne becomes a jmp, the stub's test never fires.
    core.jump(join, JOIN_LEN, stub)?;
    core.always_jump(jne)
}
