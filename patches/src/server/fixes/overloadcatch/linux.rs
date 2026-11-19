//! `overloadcatch`, Linux: each overload call (BYOND's overload call, mostly inlined, around call_proc) goes through a
//! stub before it and one after it, which raises a passed-on error as if the call had thrown it
//! ([`throw_pending`]), and the interpreter's two inlined `try` searches jump to [`caught_search`] and
//! [`recursion_search`].

use std::arch::naked_asm;

use super::{CALLS, FOUND, Found, enter, find_try, find_try_from, leave};
use crate::base::code::{self, Addr, Cond, Foreign, Stub};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// A caught error's unwinding of the value stack, after each value's `call release` (which `argload` retargets, so
/// not part of the pattern): mov esi,[running] ; add esp,10h ; jmp next. Then, with no `try` left in the proc, the
/// search for its caller's: mov eax,edx ; mov edx,[edx+4] (the caller) ; test edx,edx ; je none ; mov eax,[eax] ;
/// test byte [eax+4],10h ; je next_frame
const UNWIND: &str = "8B 35 ?? ?? ?? ?? 83 C4 10 EB ?? 8B 85 ?? ?? ?? ?? A3 ?? ?? ?? ?? 89 D0 8B 52 04 85 D2 74 ?? 8B \
                      00 F6 40 04 10 0F 84";
const UNWIND_RUNNING: usize = 2;
const CAUGHT: usize = 22;
const CAUGHT_NONE: usize = CAUGHT + 8;
const CAUGHT_NEXT: usize = CAUGHT + 17;
/// next_frame: cmp dword [edx+try],0 ; je search ; (found:) movq xmm0,[edi+4] ; mov byte [pending],1
const CAUGHT_FRAME: &str = "83 7A ?? 00 0F 84 ?? ?? ?? ?? F3 0F 7E 47 04 C6 05 ?? ?? ?? ?? 01";
const FRAME_TRY: usize = 2;
const FRAME_FOUND: usize = 10;
const FRAME_PENDING: usize = 17;
/// The search when the recursion limit is hit: mov ecx,[running] ; mov eax,ecx ; jmp check ; (loop:)
/// mov edx,[eax+4] ; test edx,edx ; je none ; mov eax,[eax] ; test byte [eax+4],10h ; jne none ;
/// mov eax,[edx+try] ; test eax,eax ; jne found ; mov eax,edx ; check: test eax,eax ; jne loop
const RECURSION: &str = "8B 0D ?? ?? ?? ?? 89 C8 EB ?? 8B 50 04 85 D2 74 ?? 8B 00 F6 40 04 10 75 ?? 8B 42 ?? 85 C0 75 ?? \
                         89 D0 85 C0 75";
const RECURSION_RUNNING: usize = 2;
const RECURSION_NONE: [usize; 2] = [16, 24];
const RECURSION_TRY: usize = 27;
const RECURSION_FOUND: usize = 31;
/// `new`'s call of New(), a proc call that passes errors on: push 0Bh ; call call_proc ; mov esi,[running] ;
/// movq xmm0,[ebp-x] ; add esp,20h. After it, the check for an error to pass on: cmp byte [pending],0 ; jne raise.
const NEW_CALL: &str = "6A 0B E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? ?? ?? 83 C4 20";
const PENDING_CHECK: &str = "80 3D ?? ?? ?? ?? 00 0F 85";
/// The raise of the pending error in the running proc: sub esp,0Ch ; mov byte [pending],0 ; push 0Ch ;
/// call __cxa_allocate_exception ; add esp,0Ch ; mov dword [eax],-1 ; movq xmm0,[error] ; movq [eax+4],xmm0 ;
/// push 0 ; push type_info ; push eax ; call __cxa_throw
const RAISE: &str = "83 EC 0C C6 05 ?? ?? ?? ?? 00 6A 0C E8 ?? ?? ?? ?? 83 C4 0C C7 00 FF FF FF FF F3 0F 7E 05 ?? ?? ?? \
                     ?? 66 0F D6 40 04 6A 00 68 ?? ?? ?? ?? 50 E8";
const RAISE_PENDING: usize = 5;
const RAISE_ALLOCATE: usize = 12;
const RAISE_ERROR: usize = 30;
const RAISE_TYPE: usize = 42;
const RAISE_THROW: usize = 47;
/// An overload call inlined from BYOND's overload call: call call_proc (regparm, or cdecl with the result's address
/// on the stack, which it pops) ; mov eax,[running] (or esi) ; add esp,args ; test eax,eax ; je done ;
/// mov eax,[eax] ; cmp byte [eax+10h],22h (the check that the caller wasn't deleted meanwhile). The last is `[]`'s.
const INLINED: [(&str, u8); 5] = [
    ("E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 20 85 C0 74 ?? 8B 00 80 78 10 22", 0),
    ("E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 20 85 F6 74 ?? 8B 06 80 78 10 22", 0),
    ("E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 2C 85 C0 74 ?? 8B 00 80 78 10 22", 4),
    ("E8 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 2C 85 F6 74 ?? 8B 06 80 78 10 22", 4),
    ("E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 83 C4 20 85 C0 0F 84 ?? ?? ?? ?? 8B 00 80 78 10 22", 0),
];
/// An inlined overload call that passes its arguments in an array: call the call_proc under call_proc (cdecl with
/// the result's address on the stack, which it pops; regparm before 516.1660) ; ... ; the array's release ; ... ;
/// the caller-deleted check. `[]=`'s (516.1660 on, then before), and the unary overloads' in builds without BYOND's
/// own overload call (516.1689).
const ARRAY: [(&str, u8); 3] = [
    (
        "E8 ?? ?? ?? ?? F3 0F 7E 45 ?? 83 C4 28 53 6A 10 68 ?? ?? ?? ?? 66 0F D6 45 ?? E8 ?? ?? ?? ?? F3 0F 7E 45 ?? 83 \
         C4 10 66 0F D6 45 ?? A1 ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 8B 00 80 78 10 22",
        4,
    ),
    (
        "E8 ?? ?? ?? ?? 83 EC 04 F3 0F 7E 85 ?? ?? ?? ?? 53 6A 10 68 ?? ?? ?? ?? 66 0F D6 85 ?? ?? ?? ?? E8 ?? ?? ?? ?? \
         F3 0F 7E 85 ?? ?? ?? ?? 83 C4 10 66 0F D6 85 ?? ?? ?? ?? A1 ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 8B 00 80 78 10 \
         22",
        0,
    ),
    (
        "E8 ?? ?? ?? ?? 83 C4 28 F3 0F 7E 85 ?? ?? ?? ?? 53 6A 00 68 ?? ?? ?? ?? 0F 29 85 ?? ?? ?? ?? E8 ?? ?? ?? ?? F3 \
         0F 7E ?? ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 10 66 0F D6 ?? ?? ?? ?? ?? 85 F6 74 ?? 8B 06 80 78 10 22",
        4,
    ),
];
/// BYOND's overload call where it isn't inlined (516.1648-1688; regparm like call_proc): its entry, and the
/// `waitfor = 0` flag it sets (or reg,10h) at most `WRAPPER_FLAG` bytes in. 516.1660-1688: push edi ;
/// movd xmm0,edx ; movd xmm1,ecx ; push esi ; punpckldq xmm0,xmm1 ; mov esi,eax ; push ebx ; sub esp,x ; ... ;
/// or ebx,10h. 516.1648-1659: push ebp ; movd xmm0,edx ; movd xmm1,ecx ; push edi ; punpckldq xmm0,xmm1 ; push esi ;
/// push ebx ; mov ebx,eax ; sub esp,x ; ... ; mov eax,ebp ; sub esp,10h ; or eax,10h.
const WRAPPERS: [(&str, &str); 2] = [
    (
        "57 66 0F 6E C2 66 0F 6E C9 56 66 0F 62 C1 89 C6 53 83 EC ?? 0F 29 44 24 08 F3 0F 7E 44 24 ?? 8B 5C 24 ?? 0F 29 \
         44 24 18",
        "83 CB 10",
    ),
    (
        "55 66 0F 6E C2 66 0F 6E C9 57 66 0F 62 C1 56 53 89 C3 81 EC ?? ?? ?? ?? 0F 29 44 24 08 8B AC 24 ?? ?? ?? ?? F3 0F \
         7E 84 24 ?? ?? ?? ?? 8B BC 24 ?? ?? ?? ?? 0F 29 44 24 18",
        "89 E8 83 EC 10 83 C8 10",
    ),
];
const WRAPPER_FLAG: usize = 0x90;
/// A stand-in `try` around `operator""`'s overload call (which keeps its errors): mov edx,[eax+try] ;
/// mov [ebp-x],edx ; lea edx,[ebp-y] ; mov [eax+try],edx
const STAND_IN: &str = "8B 50 ?? 89 55 ?? 8D 55 ?? 89 50";
const STAND_IN_TRY: usize = 2;
/// How far after a stand-in `try` its overload call is.
const STAND_IN_SPAN: usize = 0x60;

static RUNNING: Addr = Addr::new();
static CAUGHT_FOUND: Addr = Addr::new();
static CAUGHT_NO: Addr = Addr::new();
static RECURSION_YES: Addr = Addr::new();
static RECURSION_NO: Addr = Addr::new();
/// `__cxa_allocate_exception` as BYOND calls it, and the pending error's value.
static ALLOCATE: Foreign<unsafe extern "C" fn(usize) -> usize> = Foreign::new();
static ERROR: Addr = Addr::new();
/// `__cxa_throw` as BYOND calls it, and the type BYOND throws its errors as.
static THROW: Foreign<unsafe extern "C" fn(usize, usize, usize) -> !> = Foreign::new();
static TYPE_INFO: Addr = Addr::new();

/// The pending error as BYOND raises it: an exception object of -1 and the error's value, the flag cleared.
extern "C" fn exception() -> usize {
    // SAFETY: BYOND's allocation of an exception object of its size; the globals found at install.
    unsafe {
        let e = ALLOCATE.get()(12);
        code::write::<i32>(e, -1);
        code::write::<u64>(e + 4, code::read::<u64>(ERROR.get()));
        code::write::<u8>(FOUND.get().pending, 0);
        e
    }
}

/// Throws [`exception`], jumped to with an overload call's return address pushed where the call pushed it: its
/// unwind information makes it a frame the call made, so the unwinder goes on to the call's function as if the call
/// had thrown.
#[unsafe(naked)]
unsafe extern "C" fn throw_pending() {
    naked_asm!(
        ".cfi_startproc",
        "sub esp, 12",
        ".cfi_adjust_cfa_offset 12",
        "call {exception}",
        "mov dword ptr [esp], eax",
        "mov eax, dword ptr [{type_info}]",
        "mov dword ptr [esp + 4], eax",
        "mov dword ptr [esp + 8], 0",
        "call dword ptr [{throw}]",
        "ud2",
        ".cfi_endproc",
        exception = sym exception,
        type_info = sym TYPE_INFO,
        throw = sym THROW,
    )
}

/// In place of the search after a caught error (the frame in edx): [`find_try_from`], then BYOND's found or none
/// case.
#[unsafe(naked)]
unsafe extern "C" fn caught_search() {
    naked_asm!(
        "push ebx",
        "mov ebx, esp",
        "and esp, -16",
        "sub esp, 12",
        "push edx",
        "call {find}",
        "mov esp, ebx",
        "pop ebx",
        "test eax, eax",
        "jz 2f",
        "jmp dword ptr [{found}]",
        "2:",
        "jmp dword ptr [{none}]",
        find = sym find_try_from,
        found = sym CAUGHT_FOUND,
        none = sym CAUGHT_NO,
    )
}

/// In place of the search when the recursion limit is hit: [`find_try`], then BYOND's found case or its none case
/// (the running frame in ecx).
#[unsafe(naked)]
unsafe extern "C" fn recursion_search() {
    naked_asm!(
        "push ebx",
        "mov ebx, esp",
        "and esp, -16",
        "call {find}",
        "mov esp, ebx",
        "pop ebx",
        "mov ecx, dword ptr [{running}]",
        "mov ecx, dword ptr [ecx]",
        "test eax, eax",
        "jz 2f",
        "jmp dword ptr [{found}]",
        "2:",
        "jmp dword ptr [{none}]",
        find = sym find_try,
        running = sym RUNNING,
        found = sym RECURSION_YES,
        none = sym RECURSION_NO,
    )
}

/// An overload call to patch: the call, its target, the length of the whole position-independent instructions after
/// it that its stub moves, and the stack its target pops (the result's address).
struct Site {
    at: usize,
    target: usize,
    moved: usize,
    popped: u8,
}

/// The stub the call at `site` calls instead: [`enter`] with the call's stack position (registers kept for the
/// call), then the target as if called from the site.
fn before_call(site: &Site) -> Result<usize> {
    Stub::new()
        .bytes(&[0x50, 0x52, 0x51]) // push eax ; push edx ; push ecx
        .bytes(&[0x8D, 0x44, 0x24, 0x10]) // lea eax,[esp+16]
        .bytes(&[0x83, 0xEC, 0x0C]) // sub esp,12
        .bytes(&[0x50]) // push eax
        .call(enter as extern "C" fn(usize))
        .bytes(&[0x83, 0xC4, 0x10]) // add esp,16
        .bytes(&[0x59, 0x5A, 0x58]) // pop ecx ; pop edx ; pop eax
        .jmp(site.target)
        .exec()
}

/// The stub the site jumps to after the call, in place of the moved instructions: [`leave`] with the result (eax
/// points to it), then the moved instructions and back; or, to pass an error on, [`throw_pending`] with the call's
/// return address pushed at the call's stack position, as if the call had thrown it.
fn after_call(core: &Image, site: &Site) -> Result<usize> {
    let back = site.at + 5;
    let mut stub = Stub::new();
    let pass_on = stub.label();
    if site.popped != 0 {
        stub.bytes(&[0x83, 0xEC, site.popped]); // sub esp,popped (the call's stack position)
    }
    stub.bytes(&[0x50]) // push eax
        .bytes(&[0xFF, 0x70, 0x04]) // push dword [eax+4]
        .bytes(&[0xFF, 0x30]) // push dword [eax]
        .bytes(&[0x8D, 0x4C, 0x24, 0x0C]) // lea ecx,[esp+12]
        .bytes(&[0x51]) // push ecx
        .call(leave as extern "C" fn(usize, u32, u32) -> bool)
        .bytes(&[0x83, 0xC4, 0x0C]) // add esp,12
        .bytes(&[0x84, 0xC0]) // test al,al
        .bytes(&[0x58]) // pop eax
        .jcc8(Cond::NotEqual, pass_on);
    if site.popped != 0 {
        stub.bytes(&[0x83, 0xC4, site.popped]); // add esp,popped
    }
    stub.bytes(core.bytes(back, site.moved)?) // the moved instructions
        .jmp(back + site.moved)
        .bind(pass_on)
        .bytes_u32(&[0x68], back as u32, &[]) // push return address
        .jmp(throw_pending as unsafe extern "C" fn());
    stub.exec()
}

/// The length of the whole instructions at `at` that a stub can run anywhere, at least the 5 bytes of a jump: a
/// `mov` to a register from an absolute address or `[ebp+disp32]`, an `add` or `sub` of esp, and a `movq` to an XMM
/// register from `[ebp+disp]`.
fn movable(core: &Image, at: usize) -> Result<usize> {
    let mut len = 0;
    while len < 5 {
        let [op, a, b, c] = core.array::<4>(at + len)?;
        len += match (op, a, b, c & 0xC7) {
            (0xA1, ..) => 5,
            (0x8B, modrm, ..) if matches!(modrm & 0xC7, 0x05 | 0x85) => 6,
            (0x83, 0xC4 | 0xEC, ..) => 3,
            (0xF3, 0x0F, 0x7E, 0x45) => 5,
            (0xF3, 0x0F, 0x7E, 0x85) => 8,
            _ => bail!("{at:#x}: no movable instructions after an overload call"),
        };
    }
    Ok(len)
}

/// The target of the `call rel32` field at `field` as it is now (BYOND's calls into libstdc++ are relocated at load).
fn live_call(core: &Image, field: usize) -> Result<usize> {
    Ok((field + 4).wrapping_add(core.live_u32(field)? as i32 as usize))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let unwind = core.find_unique(UNWIND, "caught error's try search")?;
    // The release the `call` before the pattern made as shipped, not relied on.
    let call = core.haystack(unwind - 5, 5)?;
    ensure!(call[0] == 0xE8, "caught error's stack unwinding: no release call");
    let release = unwind.wrapping_add(i32::from_le_bytes([call[1], call[2], call[3], call[4]]) as usize);
    let running = core.ptr_at(unwind + UNWIND_RUNNING)?;
    let caught = unwind + CAUGHT;
    let caught_none = core.rel8(unwind + CAUGHT_NONE)?;
    let frame = core.rel32(unwind + CAUGHT_NEXT)?;
    core.require(frame, CAUGHT_FRAME, "caught error's try search: frame check")?;
    ensure!(core.rel32(frame + 6)? == caught, "caught error's try search: not a loop");
    let try_at = usize::from(core.u8_at(frame + FRAME_TRY)?);
    let pending = core.ptr_at(frame + FRAME_PENDING)?;
    let recursion = core.find_unique(RECURSION, "recursion limit's try search")?;
    let recursion_none = core.rel8(recursion + RECURSION_NONE[0])?;
    ensure!(
        core.ptr_at(recursion + RECURSION_RUNNING)? == running
            && core.rel8(recursion + RECURSION_NONE[1])? == recursion_none
            && usize::from(core.u8_at(recursion + RECURSION_TRY)?) == try_at,
        "recursion limit's try search: not as expected"
    );
    // Not relied on: `callleak` retargets the call. Only the error check after it is; the call's target is read as
    // shipped.
    let new = core.probe_all(NEW_CALL)?;
    let [new] = new.as_slice()[..] else { bail!("new's New() call: {} matches", new.len()) };
    let field = core.haystack(new + 3, 4)?;
    let call_proc = (new + 7).wrapping_add(i32::from_le_bytes([field[0], field[1], field[2], field[3]]) as usize);
    let check = core.find_unique_in(new, 0x60, PENDING_CHECK, "new's New() call: error check")?;
    ensure!(core.ptr_at(check + 2)? == pending, "new's New() call: another error flag");
    let raise = core.rel32(check + 9)?;
    core.require(raise, RAISE, "pending error raise")?;
    ensure!(core.ptr_at(raise + RAISE_PENDING)? == pending, "pending error raise: another flag");

    // The overload calls: the inlined ones (call_proc's regparm calls, the cdecl call_proc's, and those of the
    // call_proc under them) and the calls of BYOND's own where it isn't inlined, except `operator""`'s, inside a
    // stand-in `try`, and calls other features patch.
    let stand_in = core.find_unique(STAND_IN, "operator\"\" call's stand-in try")?;
    ensure!(usize::from(core.u8_at(stand_in + STAND_IN_TRY)?) == try_at, "operator\"\" call: another try slot");
    let text = stand_in..stand_in + STAND_IN_SPAN;
    let mut candidates = vec![];
    let mut cdecl = None;
    for (ida, popped) in INLINED {
        for &at in core.probe_all(ida)?.iter() {
            let target = core.rel32(at + 1)?;
            if popped == 0 {
                ensure!(target == call_proc, "overload call: another call_proc");
            } else {
                ensure!(*cdecl.get_or_insert(target) == target, "overload call: another cdecl call_proc");
            }
            candidates.push((at, ida.split_whitespace().count(), target, popped));
        }
    }
    let mut under = None;
    for (ida, popped) in ARRAY {
        for &at in core.probe_all(ida)?.iter() {
            let target = core.rel32(at + 1)?;
            ensure!(*under.get_or_insert(target) == target, "overload call: another call_proc under call_proc");
            candidates.push((at, ida.split_whitespace().count(), target, popped));
        }
    }
    for (entry, flag) in WRAPPERS {
        for &wrapper in core.probe_all(entry)?.iter() {
            core.require(wrapper, entry, "overload call")?;
            core.find_unique_in(wrapper, WRAPPER_FLAG, flag, "overload call: waitfor = 0 flag")?;
            for &at in core.probe_calls_to(wrapper)?.iter() {
                candidates.push((at, 5, wrapper, 0));
            }
        }
    }
    let mut sites = vec![];
    for (at, len, target, popped) in candidates {
        if text.contains(&at) {
            continue;
        }
        let moved = movable(core, at + 5)?;
        if core.claimed(at, 5 + moved) {
            continue;
        }
        core.rely(at, len.max(5 + moved))?;
        sites.push(Site { at, target, moved, popped });
    }
    ensure!(!sites.is_empty(), "no overload calls");
    for site in &sites {
        core.retarget(site.at + 1, before_call(site)?)?;
        core.jump(site.at + 5, site.moved, after_call(core, site)?)?;
    }
    core.untouched(&[(caught, 5)])?;
    core.jump(caught, 5, caught_search as unsafe extern "C" fn())?;
    core.jump(recursion, 6, recursion_search as unsafe extern "C" fn())?;
    ALLOCATE.set(live_call(core, raise + RAISE_ALLOCATE + 1)?);
    ERROR.set(core.ptr_at(raise + RAISE_ERROR)?);
    THROW.set(live_call(core, raise + RAISE_THROW + 1)?);
    TYPE_INFO.set(core.ptr_at(raise + RAISE_TYPE)?);
    RUNNING.set(running);
    CAUGHT_FOUND.set(frame + FRAME_FOUND);
    CAUGHT_NO.set(caught_none);
    RECURSION_YES.set(core.rel8(recursion + RECURSION_FOUND)?);
    RECURSION_NO.set(recursion_none);
    CALLS.set(Vec::new())?;
    FOUND.commit(cx, Found { running, try_at, pending, release: Foreign::at(release) });
    Ok(format!("{} ({} overload calls)", core.report(&[caught, recursion]), sites.len()))
}
