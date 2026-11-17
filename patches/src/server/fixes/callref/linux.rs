//! `callref`, Linux: each call() opcode reports its own three failures, and each report first calls [`release`] with
//! the arg count (ebx in the list opcode, 1 in the arglist one).
//!
//! Found from each opcode's dispatch on the callee's type, with the interpreter's frame offsets left open. The string
//! route ends `cmp callee type,26h` and then either `jne bad ; call:` (through 516.1688) or `je call ; bad:`.

use super::{Slot, args, release_first};
use crate::base::code::{Foreign, live_u32};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

/// BYOND's assign: release a slot's value and store (type, data) in it (cdecl).
type Assign = unsafe extern "C" fn(*mut Slot, u32, u32);

/// What install found: the assign, the global holding the interpreter state, and the state's callee stack position
/// and stack base offsets.
#[derive(PartialEq)]
struct Found {
    assign: usize,
    context: usize,
    pos: usize,
    base: usize,
}

static FOUND: Installed<(Foreign<Assign>, Found)> = Installed::new();

/// Each opcode's dispatch on the callee's type: movzx eax,byte [ebp+type] ; add esp,10h ; lea edi,[ebp+..] ;
/// cmp al,26h ; je call ; cmp al,45h ; je ext ; cmp al,6 ; je string ; bad: (the bad proc report)
const DISPATCH: &str = "0F B6 85 ?? ?? ?? ?? 83 C4 10 8D BD ?? ?? ?? ?? 3C 26 0F 84 ?? ?? ?? ?? 3C 45 0F 84 ?? ?? ?? ?? \
                        3C 06 0F 84 ?? ?? ?? ?? 83 EC 0C 68";
const TYPE: usize = 3;
const CALL_JE: usize = 20;
const EXT_JE: usize = 28;
const STRING_JE: usize = 36;
const BAD: usize = 40;
/// string: the callee's text to a proc ref, then push [..] ; push [..] ; push [..] ; call Assign ; add esp,10h ;
/// cmp byte [ebp+type],26h ; jne bad_proc (call: follows) or je call (bad_proc follows)
const STRING: &str = "83 EC 08 8D BD ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 57 E8 ?? ?? ?? ?? ?? ?? 6A 26 FF B5 ?? ?? ?? ?? \
                      FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 0C 6A 26 50 57 E8 ?? ?? ?? ?? ?? ?? FF B5 ?? ?? ?? ?? \
                      FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 80 BD ?? ?? ?? ?? 26 0F";
const STRING_ASSIGN_CALL: usize = 75;
const STRING_TYPE: usize = 84;
const STRING_JCC: usize = 89;
/// call: mov edx,[context] ; or byte [edx+17h],1 ; push 0 ; push 0 ; push ebx (the arg count), or push 1 ;
/// movzx eax,word [edx+pos] ; mov ecx,[edx+base] ; lea eax,[ecx+eax*8+8] (the first arg)
const CALL_LIST: &str = "8B 15 ?? ?? ?? ?? 80 4A 17 01 6A 00 6A 00 53 0F B7 42 ?? 8B 4A ?? 8D 44 C1 08";
const CALL_ARGLIST: &str = "8B 15 ?? ?? ?? ?? 80 4A 17 01 6A 00 6A 00 6A 01 0F B7 42 ?? 8B 4A ?? 8D 44 C1 08";
/// Its context global, and the two offsets after the arg count push.
const CONTEXT: usize = 2;
const POS: usize = 3;
const BASE: usize = 6;
/// ext: sub esp,0Ch ; ... ; test eax,eax ; je bad ; cmp dword [eax+8],0Ch ; jne bad ; (the call_ext report)
const EXT: &str = "83 EC 0C ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 83 78 08 0C 0F 85";
const EXT_BAD_JUMPS: [usize; 2] = [21, 31];
const EXT_REPORT: usize = 35;

/// Release the call's args and null their slots.
///
/// # Safety
/// Only from a failure report's stub: `count` is the call's arg count.
unsafe extern "C" fn release(count: u32) {
    let (assign, f) = FOUND.get();
    // SAFETY: the interpreter state, from its global, and BYOND's assign.
    unsafe {
        for slot in args(live_u32(f.context) as usize, f.base, f.pos, count) {
            assign.get()(slot, 0, 0);
        }
    }
}

/// One opcode from its dispatch at `d`: what it uses, whether it's the list opcode, and its three failure reports.
fn opcode(core: &Image, d: usize) -> Result<(Found, bool, [usize; 3])> {
    let (call, ext, string, bad) =
        (core.rel32(d + CALL_JE)?, core.rel32(d + EXT_JE)?, core.rel32(d + STRING_JE)?, d + BAD);
    let routes_ok = core.is_at(string, STRING)?
        && core.u32_at(string + STRING_TYPE)? == core.u32_at(d + TYPE)?
        && core.is_at(ext, EXT)?
        && EXT_BAD_JUMPS.iter().all(|&j| core.rel32(ext + j).is_ok_and(|t| t == bad));
    ensure!(routes_ok, "a call() opcode's failure routes are not as expected");
    // The string route's bad proc report: jumped to before `call:` (jne), or after its `je call`.
    let jcc = core.branch_at(string + STRING_JCC)?;
    let string_bad = match core.u8_at(string + STRING_JCC + 1)? {
        0x85 if jcc.next == call => jcc.target,
        0x84 if jcc.target == call => jcc.next,
        _ => return Err("a call() opcode's string route doesn't end in its call".into()),
    };
    let list = core.is_at(call, CALL_LIST)?;
    ensure!(list || core.is_at(call, CALL_ARGLIST)?, "a call() opcode's call is not as expected");
    let after_push = call + if list { 15 } else { 16 };
    let found = Found {
        assign: core.rel32(string + STRING_ASSIGN_CALL)?,
        context: core.ptr_at(call + CONTEXT)?,
        pos: usize::from(core.u8_at(after_push + POS)?),
        base: usize::from(core.u8_at(after_push + BASE)?),
    };
    Ok((found, list, [bad, ext + EXT_REPORT, string_bad]))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let dispatches = core.find_all(DISPATCH)?;
    ensure!(dispatches.len() == 2, "call() opcode: expected two, found {}", dispatches.len());
    let (a, list_a, sites_a) = opcode(core, dispatches.as_slice()[0])?;
    let (b, list_b, sites_b) = opcode(core, dispatches.as_slice()[1])?;
    ensure!(list_a != list_b && a == b, "expected two call() opcodes, a list one and an arglist one");
    // Each report's `sub esp,0Ch ; push msg` becomes a jump to: release(count) ; sub esp,0Ch ; push msg ; jmp back.
    // The extra `sub esp,0Ch` keeps the stack 16-byte aligned at the call.
    for (sites, list) in [(sites_a, list_a), (sites_b, list_b)] {
        for site in sites {
            ensure!(core.is_at(site, "83 EC 0C 68")?, "unexpected failure report at +{:#x}", site - core.base);
            // sub esp,0Ch ; push ebx (the arg count), or push 1
            let push: &[u8] = if list { &[0x83, 0xEC, 0x0C, 0x53] } else { &[0x83, 0xEC, 0x0C, 0x6A, 0x01] };
            release_first(core, site, 8, push, 0x10, release as unsafe extern "C" fn(u32))?;
        }
    }
    FOUND.commit(cx, (Foreign::at(a.assign), a));
    Ok(core.report(dispatches.as_slice()))
}
