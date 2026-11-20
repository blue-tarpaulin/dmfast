//! `callleak`: `call()` and `call_ext()` with `arglist()` or named args no longer leak references to their arguments.
//!
//! The two opcodes copy their argument lists and then fail to release the original or the temporary buffer. This
//! feature releases both originals and copied values once the calls finish, including failed and long-list calls.
//!
//! Safe: each release runs after the call is done with its own referenced copy.

#[path = "callleak_ext/mod.rs"]
mod call_ext;

use crate::base::code::{self, Foreign, Stub, live_u32};
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

pub static CALLLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "callleak",
    "call() and call_ext() arglists release their args",
    Tier::Safe,
    install!(windows: install, linux: install),
);

fn install(cx: &Ctx) -> Result<String> {
    let mut parts = cx.parts();
    parts.part(|| install_call(cx));
    parts.part(|| call_ext::install(cx));
    parts.finish()
}

/// The opcode's path route, after the callee's type check: cmp byte [ebp+callee type],26h ; jne text ;
/// cmp [world version],203h ; jb old ; mov edx,[context] ; push 0 ; push 0 ; push 1 (one arg) ; (flag the context) ;
/// movzx ecx,word [edx+pos] ; mov eax,[edx+base] ; lea eax,[eax+ecx*8] ; add eax,10h (the arglist) ; push eax ;
/// ... ; push 0Bh ; ... ; call (the copying call).
#[cfg(windows)]
const ROUTE: &str = "83 C4 08 80 BD ?? ?? ?? ?? 26 0F 85 ?? ?? ?? ?? 81 3D ?? ?? ?? ?? 03 02 00 00 0F 82 ?? ?? ?? ?? 8B \
                     15 ?? ?? ?? ?? 6A 00 6A 00 6A 01 ?? ?? ?? ?? 0F B7 4A ?? 8B 42 ?? 8D 04 C8 83 C0 10 50 FF B5 ?? \
                     ?? ?? ?? 8B 02 FF B5 ?? ?? ?? ?? 6A 00 FF B5 ?? ?? ?? ?? 6A 0B FF 70 0C FF 70 08 E8";
#[cfg(windows)]
const FIELDS: (usize, usize, usize) = (34, 54, 92);
#[cfg(windows)]
const ASSIGNED: &str = "E8 ?? ?? ?? ?? 83 C4 0C";
#[cfg(target_os = "linux")]
const ROUTE: &str = "83 C4 10 80 BD ?? ?? ?? ?? 26 0F 85 ?? ?? ?? ?? 81 3D ?? ?? ?? ?? 02 02 00 00 0F 86 ?? ?? ?? ?? A1 \
                     ?? ?? ?? ?? 83 EC 08 8D BD ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 8B 40 ?? 8B 4A 0C 8B 52 08 8D \
                     44 D8 10 50 89 F8 FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 6A 00 FF B5 ?? ?? ?? ?? 6A 0B E8";
#[cfg(target_os = "linux")]
const FIELDS: (usize, usize, usize) = (33, 58, 94);
#[cfg(target_os = "linux")]
const LOAD: usize = 46;
#[cfg(target_os = "linux")]
const ASSIGNED: &str = "E8 ?? ?? ?? ?? 83 C4 10";
const TEXT_JNE: usize = 10;

type Assign = unsafe extern "C" fn(*mut [u32; 2], u32, u32);

struct Found {
    assign: Foreign<Assign>,
    context: usize,
    pos: usize,
    base: usize,
}

static FOUND: Installed<Found> = Installed::new();

extern "C" fn release() {
    let f = FOUND.get();
    // SAFETY: the running context's stack, whose arglist slot (two after the position) the call left as it was.
    unsafe {
        let context = live_u32(f.context) as usize;
        let stack = live_u32(context + f.base) as usize;
        let slot = stack + 8 * usize::from(code::read::<u16>(context + f.pos)) + 16;
        f.assign.get()(slot as *mut [u32; 2], 0, 0);
    }
}

#[cfg(windows)]
fn position(_: &Image, at: usize) -> Result<usize> {
    Ok(at + 51)
}

#[cfg(target_os = "linux")]
fn position(core: &Image, at: usize) -> Result<usize> {
    Ok(core.find_unique_in(at + LOAD, 10, "0F B7 58", "call(src, /proc/path)(arglist()) position")? + 3)
}

fn install_call(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(ROUTE, "call(src, /proc/path)(arglist()) route")?;
    let (context, base, call) = FIELDS;
    let pos = usize::from(core.u8_at(position(core, at)?)?);
    let base = usize::from(core.u8_at(at + base)?);
    ensure!(pos != base, "call(src, /proc/path)(arglist()): unexpected context offsets");
    let text = core.rel32(at + TEXT_JNE + 2)?;
    let assigned = text - ASSIGNED.split(' ').count();
    core.require(assigned, ASSIGNED, "call(src, /proc/path)(arglist()) callee assign")?;
    FOUND.commit(
        cx,
        Found { assign: Foreign::at(core.call_target(assigned)?), context: core.ptr_at(at + context)?, pos, base },
    );
    let site = at + call;
    let stub = Stub::new()
        .call(core.call_target(site)?)
        .bytes(&[0x83, 0xEC, 0x08])
        .bytes(&[0x50])
        .bytes(&[0x52])
        .call(release as extern "C" fn())
        .bytes(&[0x5A])
        .bytes(&[0x58])
        .bytes(&[0x83, 0xC4, 0x08])
        .jmp(site + 5)
        .exec()?;
    core.jump(site, 5, stub)?;
    Ok(core.report(&[at]))
}
