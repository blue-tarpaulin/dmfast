//! `awaitstack` (forum report 2991329): a `byond,await:` `call_ext()` no longer leaves its library name or
//! `load_ext()` handle on the stack under its result (`list(7,8,9, call_ext(...)(100))` was `list(8,9,"lib",1234)`).
//!
//! An await call sleeps its proc with that operand still on the stack, and `Byond_Return` resumes it by pushing the
//! result on top. The fix hooks the resume where it decides to push: for an await sleeper, it first pops and releases
//! the operand, so the result takes its place.
//!
//! Safe: the operand is the await call's own, the topmost slot of the sleeping proc's stack, and is no longer needed.
//! Before 516.1688, an await could wake another proc's frame (forum report 2984448), whose top slot is live, so the
//! fix waits for that build.

use crate::base::Result;
use crate::base::code::{self, Foreign};
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

pub static AWAITSTACK: Feature = Feature::fix(
    Hosts::SERVER,
    "awaitstack",
    "await call_ext() leaves nothing on the stack under its result",
    Tier::Safe,
    install!(windows: install, linux: install),
)
.since(516_1688)
.fixed_in(516_1689, "https://www.byond.com/forum/post/2991329");

/// The await sleepers' type, the proc state's stack position (u16) and stack pointer offsets, and BYOND's value
/// release.
struct Await {
    sleeper: u8,
    pos: usize,
    stack: usize,
    release: Foreign<ReleaseFn>,
}

static AWAIT: Installed<Await> = Installed::new();

/// Called by the resume before it pushes the result onto `state`'s stack (which it always does for an await sleeper:
/// they're made without flags): for an await `sleeper`, pop and release the call's leftover operand.
extern "C" fn pop(state: usize, sleeper: usize) {
    let a = AWAIT.get();
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

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (sleeper, resume) = os::lookup(core)?;
    let site = core.find_unique_in(resume, 0x100, os::HOOK, "await resume push")?;
    os::check(core, resume, site)?;
    let pos = usize::from(core.u8_at(site + os::POS)?);
    let stack = usize::from(core.u8_at(site + os::STACK)?);
    AWAIT.commit(cx, Await { sleeper, pos, stack, release: Foreign::at(values::release(cx)?) });
    let stub = os::stub(core, site, pop)?;
    core.untouched(&[(site, os::HOOK_LEN)])?;
    core.jump(site, os::HOOK_LEN, stub)?;
    Ok(core.report(&[site]))
}
