//! `callref`, Windows: each failure report first calls [`release`] with the interpreter state (edi) and the arg count
//! (`[ebp-224h]`, which the fix makes the arglist opcode store as 1).

use super::{args, release_first};
use crate::base::code::{Foreign, trampoline};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// BYOND's release of a value (cdecl: type, data).
static UNREF: Foreign<unsafe extern "C" fn(u32, u32)> = Foreign::new();

/// The interpreter state's stack base and the callee's stack position.
const BASE: usize = 0x40;
const POS: usize = 0x46;

/// The opcode: inc eax ; mov [edi+14h],ax ; movzx ecx,ax ; mov eax,[edi+10h] ; mov ecx,[eax+ecx*4] (arg count) ;
/// mov eax,0FFFFh ; sub eax,ecx ; mov [ebp-224h],ecx ; add [edi+46h],ax (pop count+1 values) ; load the callee ;
/// call Unref (the callee) ; cmp al,6 ; je string ; cmp al,26h ; jne other
const OPCODE: &str = "40 66 89 47 14 0F B7 C8 8B 47 10 8B 0C 88 B8 FF FF 00 00 2B C1 89 8D DC FD FF FF 66 01 47 46 8B 47 \
                      40 0F B7 4F 46 8B B5 E8 FD FF FF 8B 14 C8 8B 4C C8 04 8B 85 EC FD FF FF 50 56 89 95 E8 FD FF FF \
                      89 8D EC FD FF FF E8 ?? ?? ?? ?? 8A 85 E8 FD FF FF 83 C4 08 3C 06 0F 84 ?? ?? ?? ?? 3C 26 0F 85";
/// Its `call Unref` field and its `jne other` field.
const UNREF_CALL: usize = 0x48;
const OTHER_JNE: usize = 0x61;
/// After the name is turned into a proc: cmp byte [ebp-218h],26h ; je ok ; push "bad proc" ; call Error ;
/// add esp,4 ; jmp exit
const BAD_PROC: &str = "80 BD E8 FD FF FF 26 0F 84 ?? ?? ?? ?? 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 04 E9 ?? ?? ?? ??";
const BAD_PROC_PUSH: usize = 13;
/// The arglist opcode, just before: mov eax,0FFFEh ; add [edi+46h],ax (pop 2 values) ; ... (as above) ; cmp al,6 ;
/// je string ; cmp al,26h ; je call ; other: cmp al,45h ; jne bad ; ... ; jne bad ; push "call_ext() ..." ;
/// call Error ; add esp,4 ; jmp exit
const ARGLIST: &str = "B8 FE FF 00 00 66 01 47 46 8B 47 40 0F B7 4F 46 8B B5 E8 FD FF FF 8B 14 C8 8B 4C C8 04 8B 85 EC \
                       FD FF FF 50 56 89 95 E8 FD FF FF 89 8D EC FD FF FF E8 ?? ?? ?? ?? 8A 85 E8 FD FF FF 83 C4 08 3C \
                       06 74 ?? 3C 26 0F 84 ?? ?? ?? ?? 3C 45 75 ?? FF B5 EC FD FF FF E8 ?? ?? ?? ?? 83 C4 04 85 C0 74 \
                       ?? 83 78 08 0C 75 ?? 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 04 E9";
/// Its `call Unref` field, `je call` field, `other:`, the three `jne bad`/`je bad` fields after it, and the
/// call_ext report's `push`.
const ARGLIST_UNREF_CALL: usize = 0x32;
const ARGLIST_CALL_JE: usize = 0x47;
const OTHER: usize = 0x4B;
const BAD_JUMPS: [usize; 3] = [3, 0x15, 0x1B];
const EXT_PUSH: usize = 0x1C;
/// Its string route: cmp byte [ebp-218h],26h ; je call ; push "bad proc" ; call Error ; add esp,4 ; jmp exit
const ARGLIST_BAD_PROC: &str = "80 BD E8 FD FF FF 26 74 ?? 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 04 E9";
const ARGLIST_BAD_PROC_PUSH: usize = 9;

/// Release the call's args and null their slots.
///
/// # Safety
/// Only from a failure report's stub: `state` is the interpreter state and `count` the call's arg count.
unsafe extern "C" fn release(state: usize, count: u32) {
    // SAFETY: the interpreter state (caller) and BYOND's release.
    unsafe {
        for slot in args(state, BASE, POS, count) {
            let [tag, data] = slot.read();
            UNREF.get()(tag, data);
            slot.write([0, 0]);
        }
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let op = core.find_unique(OPCODE, "call() opcode")?;
    let unref = core.rel32(op + UNREF_CALL)?;
    let fail = core.find_unique_in(op, 0x200, BAD_PROC, "call() bad proc report")?;
    let msg = core.ptr_at(fail + BAD_PROC_PUSH + 1)?;
    core.expect(msg, b"bad proc\0", "the error message is not \"bad proc\"")?;
    let al = core.find_unique_in(op - 0x200, 0x200, ARGLIST, "call(arglist()) opcode")?;
    let other = al + OTHER;
    let ext = other + EXT_PUSH;
    let bad = core.find_unique_in(ext, op - ext, ARGLIST_BAD_PROC, "call(arglist()) bad proc report")?
        + ARGLIST_BAD_PROC_PUSH;
    let routes_ok = core.rel32(al + ARGLIST_UNREF_CALL)? == unref
        && core.rel32(op + OTHER_JNE)? == other
        && BAD_JUMPS.iter().all(|&j| core.rel8(other + j).is_ok_and(|t| t == bad))
        && core.rel8(bad - 1)? == core.rel32(al + ARGLIST_CALL_JE)?
        && core.ptr_at(bad + 1)? == msg;
    ensure!(routes_ok, "the call() opcodes' failure routes are not as expected");
    let sites = [fail + BAD_PROC_PUSH, bad, ext];
    for site in sites {
        ensure!(core.is_at(site, "68 ?? ?? ?? ?? E8")?, "unexpected failure report at +{:#x}", site - core.base);
    }
    // The arglist opcode stores its arg count: mov dword [ebp-224h],1 ; mov eax,0FFFEh (the replaced instruction).
    let count = trampoline(&[0xC7, 0x85, 0xDC, 0xFD, 0xFF, 0xFF, 1, 0, 0, 0, 0xB8, 0xFE, 0xFF, 0, 0], al + 5)?;
    core.jump(al, 5, count)?;
    // Each report's `push msg` becomes a jump to: release(edi, [ebp-224h]) ; push msg ; jmp back.
    let push = [0xFF, 0xB5, 0xDC, 0xFD, 0xFF, 0xFF, 0x57]; // push dword [ebp-224h] ; push edi
    for site in sites {
        release_first(core, site, 5, &push, 8, release as unsafe extern "C" fn(usize, u32))?;
    }
    UNREF.set(unref);
    Ok(core.report(&[fail]))
}
