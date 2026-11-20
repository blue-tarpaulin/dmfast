//! `awaitargs` (forum report 2992896): a `byond,await:` `call_ext()` with args no longer leaks a reference to its last
//! arg, and a second await call through one `load_ext()` handle no longer fails "is not an external function".
//!
//! 516.1689 pops the library name or handle in the await call, before the library function runs, but then releases
//! the args from where they were before the pop: one slot too low, so it releases the name or handle (a second time,
//! for a handle) and never the last arg. The fix removes those pops, which keeps the args where the release expects
//! them, and pops the name or handle when the proc resumes instead, as `awaitstack` does on older builds.
//!
//! Safe: the removed pops and the resume's pop release the same slot, which nothing reads in between.

use super::awaitstack;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::{Result, ensure};

pub static AWAITARGS: Feature = Feature::fix(
    Hosts::SERVER,
    "awaitargs",
    "await call_ext() with args releases them and keeps load_ext() handles",
    Tier::Safe,
    install!(windows: install, linux: install),
)
.since(516_1689);

/// An await call's pop: mov eax,0FFFFh ; mov byte [ebp-x],y ; add word [edx+pos],ax ; movzx ecx,word [edx+pos] ;
/// mov eax,[edx+stack] ; push [eax+ecx*8+4] ; push [eax+ecx*8] ; call DecRef
#[cfg(windows)]
const POP: &str = "B8 FF FF 00 00 C6 45 ?? ?? 66 01 42 ?? 0F B7 4A ?? 8B 42 ?? FF 74 C8 04 FF 34 C8 E8";
/// Its 4-byte store of the new position (`add word`), position and stack offsets, and 7-byte pushes of the slot.
#[cfg(windows)]
const POP_STORE: usize = 9;
#[cfg(windows)]
const POP_POS: usize = 12;
#[cfg(windows)]
const POP_STACK: usize = 19;
#[cfg(windows)]
const POP_PUSHES: usize = 20;
/// movzx eax,word [ecx+pos] ; mov edx,[ecx+stack] ; sub eax,1 ; mov word [ecx+pos],ax ; movzx eax,ax ;
/// push [edx+eax*8+4] ; push [edx+eax*8] ; call DecRef
#[cfg(target_os = "linux")]
const POP: &str = "0F B7 41 ?? 8B 51 ?? 83 E8 01 66 89 41 ?? 0F B7 C0 FF 74 C2 04 FF 34 C2 E8";
#[cfg(target_os = "linux")]
const POP_STORE: usize = 10;
#[cfg(target_os = "linux")]
const POP_POS: usize = 3;
#[cfg(target_os = "linux")]
const POP_STACK: usize = 6;
#[cfg(target_os = "linux")]
const POP_PUSHES: usize = 17;

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let pops = core.find_all(POP)?;
    ensure!(!pops.is_empty(), "await call_ext(): no early pops");
    let hook = awaitstack::hook(cx)?;
    for &at in pops.iter() {
        ensure!(
            usize::from(core.u8_at(at + POP_POS)?) == hook.pos
                && usize::from(core.u8_at(at + POP_STACK)?) == hook.stack,
            "await call_ext(): an early pop isn't of the resumed stack"
        );
        // The position stays, and the DecRef (whose call other features may move) gets null.
        core.write_code(at + POP_STORE, &[0x90; 4])?;
        core.write_code(at + POP_PUSHES, &[0x6A, 0x00, 0x6A, 0x00, 0x90, 0x90, 0x90])?; // push 0 ; push 0
    }
    Ok(core.report(&[pops.as_slice(), &[hook.site]].concat()))
}
