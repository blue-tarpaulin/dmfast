//! `overloadcatch`, Windows: calls of BYOND's overload call go through [`call_stub`], which raises a passed-on error
//! where the overload call raises its own abort, and the interpreter's `try` searches call [`find_try`].

use std::arch::naked_asm;

use super::{CALLS, FOUND, Found, enter, find_try, leave};
use crate::base::code::{Addr, Foreign};
use crate::base::feature::Ctx;
use crate::base::{Result, bail, ensure};

/// The search for an error's `try`, which stops at `waitfor = 0` calls: mov eax,[running] ; test eax,eax ; je none ;
/// (loop:) mov ecx,[eax+4] (the caller) ; test ecx,ecx ; je none ; mov eax,[eax] ; test byte [eax+4],10h ;
/// jne none ; mov eax,ecx ; mov ecx,[eax+try] ; test ecx,ecx ; je loop ; mov eax,ecx ; ret ; none: xor eax,eax ; ret
const SEARCH: &str = "A1 ?? ?? ?? ?? 85 C0 74 ?? 0F 1F 80 00 00 00 00 8B 48 04 85 C9 74 ?? 8B 00 F6 40 04 10 75 ?? \
                      8B C1 8B 48 ?? 85 C9 74 ?? 8B C1 C3 33 C0 C3";
const SEARCH_RUNNING: usize = 1;
const SEARCH_TRY: usize = 35;
/// BYOND's overload call (cdecl: usr, flags, proc, 0, src, args, count; returns the value in edx:eax), which adds
/// the `waitfor = 0` flag: push ebp ; mov ebp,esp ; sub esp,14h ; mov al,[ebp+10h] ; ... ; or al,10h ; ... ; call
const CALL: &str = "55 8B EC 83 EC 14 8A 45 10 6A 00 6A 00 FF 75 28 0C 10 FF 75 24 0F B6 C0 FF 75 20 FF 75 1C FF 75 18 \
                    FF 75 14 50 FF 75 0C FF 75 08 E8";
/// Its abort when the caller's proc was deleted during the call: push 0 ; push -2 ; lea ecx,[ebp-14h] ;
/// call make_exception (thiscall: code, value) ; push throw_info ; lea eax,[ebp-14h] ; push eax ; call throw
const ABORT: &str = "6A 00 6A FE 8D 4D EC E8 ?? ?? ?? ?? 68 ?? ?? ?? ?? 8D 45 EC 50 E8";
const ABORT_MAKE: usize = 7;
const ABORT_INFO: usize = 13;
const ABORT_THROW: usize = 21;
/// A stand-in `try` around a call (`operator""`'s, which keeps its errors): mov eax,[ecx+try] ;
/// mov [ebp-x],eax ; lea eax,[ebp-y] ; mov [ecx+try],eax
const STAND_IN: &str = "8B 41 ?? 89 45 ?? 8D 45 ?? 89 41";
/// How far before its overload call a stand-in `try` is.
const STAND_IN_SPAN: usize = 0x80;
/// The interpreter's entry, which aligns its frame: push ebx ; mov ebx,esp ; sub esp,8 ; and esp,-8 ; ... ;
/// sub esp,frame
const INTERPRETER: &str = "53 8B DC 83 EC 08 83 E4 F8 83 C4 04 55 8B 6B 04 89 6C 24 04 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 \
                           00 00 00 00 50 51 53 81 EC ?? ?? 00 00";
/// Its opcode dispatch: cmp edx,last ; ja bad ; jmp [edx*4+table], the table right after its code.
const DISPATCH: &str = "81 FA ?? 01 00 00 0F 87 ?? ?? ?? ?? FF 24 95";
const DISPATCH_TABLE: usize = 15;
/// `new`'s call of New(), a proc call that passes errors on: push 0Bh ; push [eax+0Ch] ; push [eax+8] ;
/// call call_proc ; mov edi,[running] ; add esp,2Ch. After it, the check for an error to pass on:
/// cmp byte [pending],0 ; jne raise.
const NEW_CALL: &str = "6A 0B FF 70 0C FF 70 08 E8 ?? ?? ?? ?? 8B 3D ?? ?? ?? ?? 83 C4 2C";
const PENDING_CHECK: &str = "80 3D ?? ?? ?? ?? 00 0F 85";
/// The raise of the pending error in the running proc: push error ; push -1 ; lea ecx,[ebp-x] ;
/// mov byte [pending],0 ; call make_exception ; push throw_info ; lea eax,[ebp-x] ; push eax ; call throw
const RAISE: &str = "68 ?? ?? ?? ?? 6A FF 8D 8D ?? ?? ?? ?? C6 05 ?? ?? ?? ?? 00 E8 ?? ?? ?? ?? 68 ?? ?? ?? ?? 8D 85 ?? \
                     ?? ?? ?? 50 E8";
const RAISE_ERROR: usize = 1;
const RAISE_PENDING: usize = 15;
/// A caught error's unwinding of the value stack: push [eax+ecx*8+4] ; push [eax+ecx*8] ; call release ;
/// mov edx,[running] ; add esp,8 ; movzx eax,word [edx+sp] ; cmp ax,[esi+4] ; ja next
const UNWIND: &str = "FF 74 C8 04 FF 34 C8 E8 ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 83 C4 08 0F B7 42 ?? 66 3B 46 04 77";
const UNWIND_RELEASE: usize = 7;
const UNWIND_RUNNING: usize = 14;

static OVERLOAD_CALL: Addr = Addr::new();
/// The pending error's flag and value, and the overload call's abort parts.
static PENDING: Addr = Addr::new();
static ERROR: Addr = Addr::new();
static MAKE_EXCEPTION: Addr = Addr::new();
static THROW_INFO: Addr = Addr::new();
static THROW: Addr = Addr::new();

/// In place of a call of the overload call (same arguments and result): [`enter`], the call, [`leave`], then, if
/// [`leave`] says so, the pending error raised as the overload call raises its abort, so every caller unwinds as it
/// does for that.
#[unsafe(naked)]
unsafe extern "C" fn call_stub() {
    naked_asm!(
        "lea eax, [esp + 4]",
        "push eax",
        "call {enter}",
        "add esp, 4",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "push dword ptr [esp + 36]",
        "call dword ptr [{call}]",
        "add esp, 36",
        "push edx",
        "push eax",
        "lea ecx, [esp + 12]",
        "push ecx",
        "call {leave}",
        "add esp, 4",
        "test al, al",
        "pop eax",
        "pop edx",
        "jnz 2f",
        "ret",
        // The exception object, on this stack, which stays until the catch is done.
        "2:",
        "sub esp, 16",
        "mov eax, dword ptr [{pending}]",
        "mov byte ptr [eax], 0",
        "push dword ptr [{error}]",
        "push -1",
        "lea ecx, [esp + 8]",
        "call dword ptr [{make}]",
        "push dword ptr [{info}]",
        "lea eax, [esp + 4]",
        "push eax",
        "call dword ptr [{throw}]",
        enter = sym enter,
        call = sym OVERLOAD_CALL,
        leave = sym leave,
        pending = sym PENDING,
        error = sym ERROR,
        make = sym MAKE_EXCEPTION,
        info = sym THROW_INFO,
        throw = sym THROW,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let search = core.find_unique(SEARCH, "try search")?;
    let running = core.ptr_at(search + SEARCH_RUNNING)?;
    let try_at = usize::from(core.u8_at(search + SEARCH_TRY)?);
    let call = core.find_unique(CALL, "overload call")?;
    let abort = core.find_unique_in(call, 0x90, ABORT, "overload call: abort")?;
    let interpreter = core.find_unique(INTERPRETER, "interpreter")?;
    let dispatch = core.find_unique(DISPATCH, "opcode dispatch")?;
    let end = core.ptr_at(dispatch + DISPATCH_TABLE)?;
    ensure!((interpreter..end).contains(&dispatch), "opcode dispatch outside the interpreter");
    // Not relied on: `callleak` retargets the call. Only the error check after it is.
    let new: Vec<usize> =
        core.probe_all(NEW_CALL)?.iter().copied().filter(|a| (interpreter..end).contains(a)).collect();
    let [new] = new[..] else { bail!("new's New() call: {} matches", new.len()) };
    let check = core.find_unique_in(new, 0x60, PENDING_CHECK, "new's New() call: error check")?;
    let pending = core.ptr_at(check + 2)?;
    let raise = core.rel32(check + 9)?;
    core.require(raise, RAISE, "pending error raise")?;
    ensure!(core.ptr_at(raise + RAISE_PENDING)? == pending, "pending error raise: another flag");
    let unwind = core.find_unique(UNWIND, "caught error's stack unwinding")?;
    ensure!(core.ptr_at(unwind + UNWIND_RUNNING)? == running, "caught error's stack unwinding: another frame global");
    let release = core.call_target(unwind + UNWIND_RELEASE)?;
    let stand_ins = core.probe_all(STAND_IN)?;
    let mut text = vec![];
    let mut sites = vec![];
    for &site in core.probe_calls_to(call)?.iter() {
        if let Some(&at) = stand_ins.iter().find(|&&at| at < site && site - at < STAND_IN_SPAN) {
            // operator"": its stand-in `try` is the running frame's.
            ensure!(usize::from(core.u8_at(at + 2)?) == try_at, "operator\"\" call: another try slot");
            text.push(site);
        } else if !core.claimed(site, 5) {
            sites.push(site);
        }
    }
    ensure!(text.len() == 1, "operator\"\" call: {} candidates", text.len());
    ensure!(!sites.is_empty(), "no overload calls");
    for &site in &sites {
        core.rely(site, 5)?;
        core.retarget(site + 1, call_stub as unsafe extern "C" fn())?;
    }
    core.redirect_calls(search, find_try as extern "C" fn() -> usize)?;
    OVERLOAD_CALL.set(call);
    PENDING.set(pending);
    ERROR.set(core.ptr_at(raise + RAISE_ERROR)?);
    MAKE_EXCEPTION.set(core.call_target(abort + ABORT_MAKE)?);
    THROW_INFO.set(core.ptr_at(abort + ABORT_INFO)?);
    THROW.set(core.call_target(abort + ABORT_THROW)?);
    CALLS.set(Vec::new())?;
    FOUND.commit(cx, Found { running, try_at, pending, release: Foreign::at(release) });
    Ok(format!("{} ({} overload calls)", core.report(&[search, call]), sites.len()))
}
