//! `callref`, Linux: each call() opcode reports its own three failures, and each report first calls [`release`] with
//! the arg count (ebx in the list opcode, 1 in the arglist one).

use super::{Slot, args, release_first};
use crate::base::code::{Foreign, live_u32};
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, bail};

/// BYOND's assign: release a slot's value and store (type, data) in it (cdecl).
type Assign = unsafe extern "C" fn(*mut Slot, u32, u32);

/// What install found: the assign, the global holding the interpreter state, and the state's callee stack position
/// and stack base offsets.
struct Found {
    assign: Foreign<Assign>,
    context: usize,
    pos: usize,
    base: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// The list opcode: push [ebp-48Ch] ; push [ebp-490h] ; push [ebp-4FCh] ; call Assign ; add esp,10h ;
/// cmp byte [ebp-468h],26h ; jne bad_proc ; call: mov edx,[context] ; or byte [edx+17h],1 ; push 0 ; push 0 ;
/// push ebx (the arg count) ; movzx eax,word [edx+pos] ; mov ecx,[edx+base] ; lea eax,[ecx+eax*8+8] (the first arg)
const OPCODE: &str = "FF B5 74 FB FF FF FF B5 70 FB FF FF FF B5 04 FB FF FF E8 ?? ?? ?? ?? 83 C4 10 80 BD 98 FB FF FF 26 \
                      0F 85 ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 80 4A 17 01 6A 00 6A 00 53 0F B7 42 ?? 8B 4A ?? 8D 44 C1 08";
/// Its `call Assign` field, `cmp`, the `call:` route, the context global, and the two offsets.
const ASSIGN_CALL: usize = 19;
const CMP: usize = 26;
const CALL_ROUTE: usize = 39;
const CONTEXT: usize = 41;
const POS: usize = 57;
const BASE: usize = 60;
/// Each opcode's dispatch on the callee's type: cmp al,26h ; je call ; cmp al,45h ; je ext ; cmp al,6 ; je string ;
/// bad: (the bad proc report)
const DISPATCH: &str = "0F B6 85 98 FB FF FF 83 C4 10 8D BD 70 FB FF FF 3C 26 0F 84 ?? ?? ?? ?? 3C 45 0F 84 ?? ?? ?? ?? \
                        3C 06 0F 84 ?? ?? ?? ?? 83 EC 0C 68";
const CALL_JE: usize = 20;
const EXT_JE: usize = 28;
const BAD: usize = 40;
/// The string route's end, before `call:`: cmp byte [ebp-468h],26h ; jne (bad proc report)
const STRING_END: &str = "80 BD 98 FB FF FF 26 0F 85";
/// ext: sub esp,0Ch ; ... ; test eax,eax ; je bad ; cmp dword [eax+8],0Ch ; jne bad ; (the call_ext report)
const EXT: &str = "83 EC 0C ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 83 78 08 0C 0F 85";
const EXT_BAD_JUMPS: [usize; 2] = [21, 31];
const EXT_REPORT: usize = 35;

/// Release the call's args and null their slots.
///
/// # Safety
/// Only from a failure report's stub: `count` is the call's arg count.
unsafe extern "C" fn release(count: u32) {
    let f = FOUND.get();
    // SAFETY: the interpreter state, from its global, and BYOND's assign.
    unsafe {
        for slot in args(live_u32(f.context) as usize, f.base, f.pos, count) {
            f.assign.get()(slot, 0, 0);
        }
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(OPCODE, "call() opcode")?;
    let [pos] = core.array(at + POS)?;
    let [base] = core.array(at + BASE)?;
    let found = Found {
        assign: Foreign::at(core.rel32(at + ASSIGN_CALL)?),
        context: core.ptr_at(at + CONTEXT)?,
        pos: usize::from(pos),
        base: usize::from(base),
    };
    // Each report and whether it's the list opcode's.
    let mut sites = Vec::new();
    for d in core.find_all(DISPATCH)? {
        let (call, ext, bad) = (core.rel32(d + CALL_JE)?, core.rel32(d + EXT_JE)?, d + BAD);
        let routes_ok = core.is_at(call - 13, STRING_END)?
            && core.is_at(ext, EXT)?
            && EXT_BAD_JUMPS.iter().all(|&j| core.rel32(ext + j).is_ok_and(|t| t == bad));
        if !routes_ok {
            bail!("a call() opcode's failure routes are not as expected");
        }
        let list = call == at + CALL_ROUTE;
        sites.extend([(bad, list), (ext + EXT_REPORT, list), (core.rel32(call - 4)?, list)]);
    }
    if sites.len() != 6 || sites.iter().filter(|s| s.1).count() != 3 {
        bail!("expected two call() opcodes");
    }
    // Each report's `sub esp,0Ch ; push msg` becomes a jump to: release(count) ; sub esp,0Ch ; push msg ; jmp back.
    // The extra `sub esp,0Ch` keeps the stack 16-byte aligned at the call.
    for (site, list) in sites {
        if !core.is_at(site, "83 EC 0C 68")? {
            bail!("unexpected failure report at +{:#x}", site - core.base);
        }
        // sub esp,0Ch ; push ebx (the arg count), or push 1
        let push: &[u8] = if list { &[0x83, 0xEC, 0x0C, 0x53] } else { &[0x83, 0xEC, 0x0C, 0x6A, 0x01] };
        release_first(core, site, 8, push, 0x10, release as unsafe extern "C" fn(u32))?;
    }
    FOUND.commit(cx, found);
    Ok(format!("at +{:#x}", at + CMP - core.base))
}
