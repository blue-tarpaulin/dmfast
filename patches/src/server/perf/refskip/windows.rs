//! `refskip`, Windows: byondcore's calls to `IncRefCount` and `DecRefCount` (cdecl: type, data) go to a stub that
//! runs the functions themselves only for the types it can't answer. The functions stay as they are, so tools that
//! find them by their bytes (auxtools) still do, and their calls run them as before.

use std::arch::naked_asm;

use crate::base::code::Addr;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The prologue and dispatch both share (DecRef pushes edi too): push ebp ; mov ebp,esp ; push -1 ; push handler ;
/// mov eax,fs:[0] ; push eax ; push esi ; [push edi] ; mov eax,[cookie] ; xor eax,ebp ; push eax ;
/// lea eax,[ebp-0Ch] ; mov fs:[0],eax ; mov ecx,[ebp+8] ; movzx eax,cl ; cmp eax,LAST ; ja default ;
/// movzx eax,byte [eax+INDEX] ; jmp [eax*4+TARGETS]
const INC: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 56 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 \
                   8B 4D 08 0F B6 C1 83 F8 ?? 0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ??";
const DEC: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 \
                   00 8B 4D 08 0F B6 C1 83 F8 ?? 0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ??";
/// What a case ends with: restore the exception list, pop the saved registers, return. A do-nothing case is only
/// this; a helper case is push [ebp+0Ch] ; call helper ; add esp,4 first.
const INC_RETURN: &str = "8B 4D F4 64 89 0D 00 00 00 00 59 5E 8B E5 5D C3";
const DEC_RETURN: &str = "8B 4D F4 64 89 0D 00 00 00 00 59 5F 5E 8B E5 5D C3";
const HELPER: &str = "FF 75 0C E8 ?? ?? ?? ?? 83 C4 04";
const HELPER_LEN: usize = 11;

/// What the stub does per type byte: return at once, run the original, or call this helper with the data.
const NOTHING: usize = 0;
const ORIGINAL: usize = 1;

/// One function's actions by type byte, and the function.
#[repr(C)]
struct Dispatch {
    actions: [Addr; 256],
    original: Addr,
}

const _: () = assert!(std::mem::offset_of!(Dispatch, original) == 1024);

static INC_AT: Dispatch = Dispatch::new();
static DEC_AT: Dispatch = Dispatch::new();

impl Dispatch {
    const fn new() -> Dispatch {
        Dispatch { actions: [const { Addr::new() }; 256], original: Addr::new() }
    }
}

/// The stub one function's callers call: [esp+4] = type (low byte), [esp+8] = data. Clobbers what a cdecl call may.
macro_rules! skip_stub {
    ($name:ident, $d:ident) => {
        /// # Safety
        /// As the function: called with its arguments.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "movzx eax, byte ptr [esp+4]",
                "mov eax, dword ptr [{d}+eax*4]",
                "cmp eax, 1",
                "jb 2f",
                "je 3f",
                "push dword ptr [esp+8]",
                "call eax",
                "add esp, 4",
                "2:",
                "ret",
                "3:",
                "jmp dword ptr [{d}+1024]",
                d = sym $d,
            )
        }
    };
}

skip_stub!(inc_stub, INC_AT);
skip_stub!(dec_stub, DEC_AT);

/// The function whose default case reports `name`: the signature, and a mention of the name's string. Relies on the
/// matched prologue and dispatch, so a feature rewriting them conflicts.
fn find(core: &Image, ida: &str, name: &[u8], what: &str) -> Result<usize> {
    let s = core.find_data(name)?.unique(what)?;
    let f = core.probe_unique(ida, what, |f| core.mentions(f, 0x400, s).unwrap_or(false))?;
    core.rely(f, ida.split_whitespace().count())?;
    Ok(f)
}

/// Reads `f`'s dispatch (`push edi` shifts it by `extra`) into `d`'s actions: a case that only returns does
/// nothing, one that only calls a helper with the data calls it, any other runs the original.
fn dispatch(core: &Image, f: usize, extra: usize, ret: &str, d: &Dispatch) -> Result<(usize, usize)> {
    let last = core.u8_at(f + 0x2B + extra)? as usize;
    let index = core.ptr_at(f + 0x35 + extra)?;
    let targets = core.ptr_at(f + 0x3C + extra)?;
    let (mut nothing, mut helpers) = (0, 0);
    for (t, action) in d.actions.iter().enumerate() {
        let a = if t > last {
            ORIGINAL
        } else {
            let case = core.ptr_at(targets + 4 * core.u8_at(index + t)? as usize)?;
            if core.is_at(case, ret)? {
                nothing += 1;
                NOTHING
            } else if core.is_at(case, HELPER)? && core.is_at(case + HELPER_LEN, ret)? {
                helpers += 1;
                core.call_target(case + 3)?
            } else {
                ORIGINAL
            }
        };
        action.set(a);
    }
    ensure!(d.actions[0].get() == NOTHING && d.actions[0x2A].get() == NOTHING, "reference count: null or numbers act");
    Ok((nothing, helpers))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let inc = find(core, INC, b"IncRefCount\0", "IncRefCount")?;
    let dec = find(core, DEC, b"DecRefCount\0", "DecRefCount")?;
    let (n, h) = dispatch(core, inc, 0, INC_RETURN, &INC_AT)?;
    dispatch(core, dec, 1, DEC_RETURN, &DEC_AT)?;
    INC_AT.original.set(inc);
    DEC_AT.original.set(dec);
    // A call inside code an earlier feature rewrote or matched keeps calling the function.
    let retarget = |f: usize, stub: *const ()| -> Result<usize> {
        let calls: Vec<usize> = core.calls_to(f)?.iter().copied().filter(|&c| !core.claimed(c, 5)).collect();
        ensure!(!calls.is_empty(), "reference count: no callers");
        for &c in &calls {
            core.retarget(c + 1, stub)?;
        }
        Ok(calls.len())
    };
    let (inc_calls, dec_calls) = (retarget(inc, inc_stub as *const ())?, retarget(dec, dec_stub as *const ())?);
    Ok(format!(
        "at +{:#x}, +{:#x}: {inc_calls} and {dec_calls} calls; {n} types return at once, {h} call their helper",
        inc - core.base,
        dec - core.base
    ))
}
