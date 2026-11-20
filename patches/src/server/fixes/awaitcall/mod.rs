//! `await` (forum reports 2991329 and 2992896): await `call_ext()` no longer leaves its library name or handle under
//! its result, leaks its last argument, or breaks a `load_ext()` handle's second call.
//!
//! An await call sleeps its proc with its library name or handle still on the stack, and `Byond_Return` resumes it by
//! pushing the result on top. The resume hook pops and releases that operand, so the result takes its place. On
//! 516.1689, the call also has an early pop that releases the wrong slot; the feature removes that pop's release and
//! lets the resume hook handle it instead.
//!
//! Safe: the operand is the await call's own, the topmost slot of the sleeping proc's stack, and is no longer needed.
//! Before 516.1688, an await could wake another proc's frame (forum report 2984448), whose top slot is live.

use crate::base::Result;
use crate::base::code::{self, Foreign};
use crate::base::ensure;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::sync::Installed;
use crate::server::values::{self, ReleaseFn, Slot};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

pub static AWAIT: Feature = Feature::fix(
    Hosts::SERVER,
    "await",
    "await call_ext() releases its operand and arguments",
    Tier::Safe,
    install!(windows: install, linux: install),
)
.since(516_1688);

/// An await call's pop: mov eax,0FFFFh ; mov byte [ebp-x],y ; add word [edx+pos],ax ; movzx ecx,word [edx+pos] ;
/// mov eax,[edx+stack] ; push [eax+ecx*8+4] ; push [eax+ecx*8] ; call DecRef
#[cfg(windows)]
const POP: &str = "B8 FF FF 00 00 C6 45 ?? ?? 66 01 42 ?? 0F B7 4A ?? 8B 42 ?? FF 74 C8 04 FF 34 C8 E8";
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

/// The await sleepers' type, the proc state's stack position (u16) and stack pointer offsets, and BYOND's value
/// release.
struct Await {
    sleeper: u8,
    pos: usize,
    stack: usize,
    release: Foreign<ReleaseFn>,
}

static AWAIT_STATE: Installed<Await> = Installed::new();

/// The resume hook: where it is, and the stack position and pointer offsets it reads.
struct Hook {
    site: usize,
    pos: usize,
    stack: usize,
}

/// Called by the resume before it pushes the result onto `state`'s stack (which it always does for an await sleeper:
/// they're made without flags): for an await `sleeper`, pop and release the call's leftover operand.
extern "C" fn pop(state: usize, sleeper: usize) {
    let a = AWAIT_STATE.get();
    // SAFETY: the resume's proc state and sleeper; the stack's slots below its position are live values.
    unsafe {
        if code::read::<u8>(sleeper) != a.sleeper {
            return;
        }
        let Some(top) = code::read::<u16>(state + a.pos).checked_sub(1) else {
            return;
        };
        code::write::<u16>(state + a.pos, top);
        let slot = code::read::<usize>(state + a.stack) + usize::from(top) * size_of::<Slot>();
        a.release.get()(slot as *mut Slot, 1);
    }
}

/// Finds `Byond_Return`'s resume of an await sleeper and hooks it with [`pop`].
fn hook(cx: &Ctx) -> Result<Hook> {
    let core = cx.core();
    let (sleeper, resume) = os::lookup(core)?;
    let site = core.find_unique_in(resume, 0x100, os::HOOK, "await resume push")?;
    os::check(core, resume, site)?;
    let pos = usize::from(core.u8_at(site + os::POS)?);
    let stack = usize::from(core.u8_at(site + os::STACK)?);
    AWAIT_STATE.commit(cx, Await { sleeper, pos, stack, release: Foreign::at(values::release(cx)?) });
    let stub = os::stub(core, site, pop)?;
    core.untouched(&[(site, os::HOOK_LEN)])?;
    core.jump(site, os::HOOK_LEN, stub)?;
    Ok(Hook { site, pos, stack })
}

/// Installs the resume stack fix on 516.1688, and the 516.1689+ early-pop fix plus the shared resume hook later.
fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let pops = core.find_all(POP)?;
    let hook = hook(cx)?;
    if pops.is_empty() {
        return Ok(core.report(&[hook.site]));
    }
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
