//! `stdio` and `ftell`, Windows: byondcore's lexer read and .dmb writers through unlocked fast paths, and the string
//! writer's `ftell` answered from the position those writers keep.
//!
//! The fast paths are `_fgetc_nolock` and `_fputc_nolock` on the UCRT `FILE` ([`ucrt_file_layout`] checks it) and
//! call ucrtbase only to refill or flush. The int writer is replaced whole and returns nothing: its callers ignore
//! eax.
//!
//! The string writer asks `ftell` (a syscall) before every 2-byte write. Once a real `ftell` gives the stream's
//! position, every byte written through the fast paths moves it on, and with all four writers replaced nothing else
//! writes in the writer. The position is kept across the string table save loop's writer calls, as that loop calls
//! only the writer and a pure checksum, and dropped on entry to the save function and to the writer from anywhere
//! else.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};

use super::verify;
use crate::base::code::{Addr, Foreign, hx, read};
use crate::base::feature::{Ctx, Parts};
use crate::base::image::Image;
use crate::base::windows::ucrt::ucrt_file_layout;
use crate::base::{Result, bail, ensure};
use crate::compiler::string_table;

type GetcFn = unsafe extern "C" fn(*mut File) -> i32;
type FputcFn = unsafe extern "C" fn(i32, *mut File) -> i32;
type FtellFn = unsafe extern "C" fn(*mut File) -> i32;
type LexerGetcFn = unsafe extern "thiscall" fn(usize) -> i32;
type IntWriterFn = unsafe extern "C" fn(u32, *mut File);

/// The UCRT `FILE` fields the fast paths use.
#[repr(C)]
struct File {
    ptr: *mut u8,
    _base: *mut u8,
    cnt: i32,
}

/// ucrtbase's `getc`, `fputc`, and `ftell`, which the fast paths fall back to.
static GETC: Foreign<GetcFn> = Foreign::new();
static FPUTC: Foreign<FputcFn> = Foreign::new();
static FTELL: Foreign<FtellFn> = Foreign::new();

/// Every .dmb writer goes through the fast paths (`stdio` commits it), which `ftell` relies on.
static ALL_WRITERS: AtomicBool = AtomicBool::new(false);

/// The stream whose position is [`POS`]; 0: none (BYOND never passes a null stream). The entry hooks clear it.
static TRACKED: AtomicUsize = AtomicUsize::new(0);
static POS: AtomicU32 = AtomicU32::new(0);
/// The return addresses of the save loop's two string writer calls.
static LOOP_RET_A: Addr = Addr::new();
static LOOP_RET_B: Addr = Addr::new();
/// The save function and the string writer, past their entry hooks.
static SAVE: Addr = Addr::new();
static WRITER: Addr = Addr::new();

/// The lexer's stream, at `[ecx+8]`.
const LEXER_STREAM: usize = 8;

/// `checked` in verify mode, else `fast`: a hook's `CHECK` versions, chosen once at install.
fn pick<F>(checked: F, fast: F) -> F {
    if crate::base::driver::verify() { checked } else { fast }
}

/// `n` bytes written to `fp`: a tracked position moves on.
fn advance(fp: *mut File, n: u32) {
    if fp as usize == TRACKED.load(Relaxed) {
        POS.store(POS.load(Relaxed).wrapping_add(n), Relaxed);
    }
}

/// `_fputc_nolock`: ucrtbase's `fputc` only to flush.
///
/// # Safety
/// `fp` is a live UCRT stream.
#[inline(always)]
unsafe fn put(c: i32, fp: *mut File) -> i32 {
    // SAFETY: a live stream (caller), with `_cnt` bytes of room at `_ptr`; ucrtbase's `fputc` (install).
    unsafe {
        let f = &mut *fp;
        if f.cnt <= 0 {
            return FPUTC.get()(c, fp);
        }
        f.cnt -= 1;
        *f.ptr = c as u8;
        f.ptr = f.ptr.add(1);
        c & 0xFF
    }
}

/// Replaces the lexer's character read: `_fgetc_nolock`, ucrtbase's `getc` only to refill.
///
/// # Safety
/// `lexer` is BYOND's lexer, its stream live (BYOND passes it).
unsafe extern "thiscall" fn lexer_getc<const CHECK: bool>(lexer: usize) -> i32 {
    // SAFETY: the lexer's live stream (caller), with `_cnt` bytes at `_ptr`; ucrtbase's `getc` (install).
    unsafe {
        let fp = read::<*mut File>(lexer + LEXER_STREAM);
        if CHECK {
            verify::check_thread(fp as usize);
        }
        let f = &mut *fp;
        if f.cnt <= 0 {
            return GETC.get()(fp);
        }
        f.cnt -= 1;
        let c = *f.ptr;
        f.ptr = f.ptr.add(1);
        i32::from(c)
    }
}

/// Replaces the .dmb int writer: `value`'s 4 little-endian bytes.
///
/// # Safety
/// `fp` is a live UCRT stream (BYOND passes it).
unsafe extern "C" fn int_writer<const CHECK: bool>(value: u32, fp: *mut File) {
    if CHECK {
        verify::check_thread(fp as usize);
    }
    advance(fp, 4);
    for b in value.to_le_bytes() {
        // SAFETY: a live stream (caller).
        unsafe { put(i32::from(b), fp) };
    }
}

/// Replaces the other writers' `fputc` calls.
///
/// # Safety
/// `fp` is a live UCRT stream (BYOND passes it).
unsafe extern "C" fn fputc<const CHECK: bool>(c: i32, fp: *mut File) -> i32 {
    if CHECK {
        verify::check_thread(fp as usize);
    }
    advance(fp, 1);
    // SAFETY: a live stream (caller).
    unsafe { put(c, fp) }
}

/// Replaces the string writer's `ftell` calls: the tracked position, else ucrtbase's answer, which is tracked from
/// then on. `CHECK`: always ucrtbase's, compared with the tracked one.
///
/// # Safety
/// `fp` is a live UCRT stream (BYOND passes it).
unsafe extern "C" fn cached_ftell<const CHECK: bool>(fp: *mut File) -> i32 {
    let tracked = (fp as usize == TRACKED.load(Relaxed)).then(|| POS.load(Relaxed) as i32);
    if !CHECK && let Some(pos) = tracked {
        return pos;
    }
    // SAFETY: ucrtbase's `ftell` (install) on a live stream (caller).
    let real = unsafe { FTELL.get()(fp) };
    if real != -1 {
        POS.store(real as u32, Relaxed);
        TRACKED.store(fp as usize, Relaxed);
    }
    if let Some(pos) = tracked {
        verify::compare(pos, real);
    }
    real
}

/// Replaces the save function's entry: drops the tracked position, then runs the function.
///
/// # Safety
/// Only jumped to from the save function's entry.
#[unsafe(naked)]
unsafe extern "C" fn save_entry() {
    naked_asm!(
        "mov dword ptr [{tracked}], 0",
        "jmp dword ptr [{save}]",
        tracked = sym TRACKED,
        save = sym SAVE,
    )
}

/// Replaces the string writer's entry: drops the tracked position unless called from the save loop, then runs the
/// writer. ecx is free: the writer is cdecl.
///
/// # Safety
/// Only jumped to from the string writer's entry.
#[unsafe(naked)]
unsafe extern "C" fn writer_entry() {
    naked_asm!(
        "mov ecx, dword ptr [esp]",
        "cmp ecx, dword ptr [{ret_a}]",
        "je 2f",
        "cmp ecx, dword ptr [{ret_b}]",
        "je 2f",
        "mov dword ptr [{tracked}], 0",
        "2:",
        "jmp dword ptr [{writer}]",
        ret_a = sym LOOP_RET_A,
        ret_b = sym LOOP_RET_B,
        tracked = sym TRACKED,
        writer = sym WRITER,
    )
}

/// The lexer's character read: push [ecx+8] ; call [getc] ; add esp,4 ; ret
fn lexer(core: &Image) -> Result<()> {
    let slot = core.ucrt_slot("getc")?;
    let f = core.find_unique(&format!("FF 71 08 FF 15 {} 83 C4 04 C3", hx(slot)), "lexer getc wrapper")?;
    GETC.set(core.live_ptr(slot)?);
    core.jump(f, 5, pick(lexer_getc::<true> as LexerGetcFn, lexer_getc::<false> as LexerGetcFn))
}

/// The .dmb int writer (`{f}`: `fputc`'s import slot): push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+8] ;
/// push esi ; mov esi,[ebp+0Ch] ; push edi ; mov edi,[fputc] ; 4x (push esi ; push ebx ; call edi) with shr ebx,8 ;
/// add esp,20h ; pop edi ; pop esi ; pop ebx ; pop ebp ; ret
const INT_WRITER: &str = "55 8B EC 53 8B 5D 08 56 8B 75 0C 57 8B 3D {f} 56 53 FF D7 C1 EB 08 56 53 FF D7 C1 EB 08 \
                          56 53 FF D7 56 C1 EB 08 53 FF D7 83 C4 20 5F 5E 5B 5D C3";

/// The other .dmb writers (`f`: `fputc`'s import slot): name, pattern, and where its `call [fputc]` sites are. The
/// short writers end alike: call [fputc] ; push [ebp+0Ch] ; shr ebx,8 ; push ebx ; call [fputc] ; add esp,10h ;
/// pop ebx ; pop ebp ; ret
fn fputc_writers(f: &str) -> [(&'static str, String, &'static [usize]); 3] {
    let tail = format!("FF 15 {f} FF 75 0C C1 EB 08 53 FF 15 {f} 83 C4 10 5B 5D C3");
    [
        // mov eax,[ebp+8] ; xor eax,[ebp+10h] ; push ebx ; push [ebp+0Ch] ; movzx ebx,ax ; push ebx
        ("xor short writer", format!("8B 45 08 33 45 10 53 FF 75 0C 0F B7 D8 53 {tail}"), &[14, 27]),
        // push ebx ; push [ebp+0Ch] ; movzx ebx,word [ebp+8] ; push ebx
        ("short writer", format!("53 FF 75 0C 0F B7 5D 08 53 {tail}"), &[9, 22]),
        // movzx ecx,byte [esi+eax] ; movzx eax,bl ; xor ecx,eax ; push ecx ; call [fputc] ; inc esi ; add esp,8 ;
        // add bl,9
        ("string byte writer", format!("0F B6 0C 06 0F B6 C3 33 C8 51 FF 15 {f} 46 83 C4 08 80 C3 09"), &[10]),
    ]
}

/// The int writer and [`fputc_writers`], each a part, on ucrtbase's `fputc` at `slot`; whether all installed.
fn writers(core: &Image, parts: &mut Parts, slot: usize) -> bool {
    let f = hx(slot);
    let mut all = parts.named("int writer", || {
        let w = core.find_unique(&INT_WRITER.replace("{f}", &f), "int writer")?;
        core.jump(w, 5, pick(int_writer::<true> as IntWriterFn, int_writer::<false> as IntWriterFn))
    });
    let hook = pick(fputc::<true> as FputcFn, fputc::<false> as FputcFn);
    for (name, ida, calls) in fputc_writers(&f) {
        all &= parts.named(name, || {
            let at = core.find_unique(&ida, name)?;
            calls.iter().try_for_each(|&c| core.call_iat_to(at + c, slot, hook))
        });
    }
    all
}

/// The lexer read and the .dmb writers, each a part.
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    ucrt_file_layout()?;
    let mut parts = cx.parts();
    parts.named("getc", || lexer(core));
    match core.ucrt_slot("fputc").and_then(|slot| Ok((slot, core.live_ptr(slot)?))) {
        Ok((slot, crt)) => {
            FPUTC.set(crt);
            let all = writers(core, &mut parts, slot);
            cx.on_commit(move || ALL_WRITERS.store(all, Relaxed));
        }
        Err(e) => parts.skip(format!("fputc ({e})")),
    }
    parts.finish()
}

/// The string writer (`{t}`: `ftell`'s import slot): push ebp ; mov ebp,esp ; sub esp,8 ; push ebx ;
/// mov ebx,[ebp+8] ; push esi ; push edi ; mov edi,[ebp+0Ch] ; test ebx,ebx ; jne .. ; movzx esi,word [ebp+10h] ;
/// push edi ; call [ftell]
const STRING_WRITER: &str = "55 8B EC 83 EC 08 53 8B 5D 08 56 57 8B 7D 0C 85 DB 75 ?? 0F B7 75 10 57 FF 15 {t}";
/// Its end: pop edi ; pop esi ; pop ebx ; mov esp,ebp ; pop ebp ; ret, then the next function's push ebp.
const WRITER_END: &str = "5F 5E 5B 8B E5 5D C3 55";
/// How far into the writer its end is looked for, and the writer's bytes in [`WRITER_END`].
const WRITER_MAX: usize = 0x100;
const END_LEN: usize = 7;
/// Its `ftell` calls: push edi ; call [ftell], 4 of them.
const FTELL_CALL: &str = "57 FF 15 {t}";
const FTELL_CALLS: usize = 4;
/// The entry both hooks move: push ebp ; mov ebp,esp ; sub esp,imm8.
const ENTRY: usize = 6;
/// How far into the save function its string writer calls are looked for.
const SAVE_LEN: usize = 0x404;

/// The string writer's `ftell` calls through [`cached_ftell`]. Needs every writer fast path from [`install`].
pub(super) fn install_ftell(cx: &Ctx) -> Result<String> {
    ensure!(ALL_WRITERS.load(Relaxed), "needs all stdio writer fast paths");
    let core = cx.core();
    let save = string_table::save(cx)?;
    let slot = core.iat_slot("ftell")?;
    let t = hx(slot);
    let writer = core.find_unique(&STRING_WRITER.replace("{t}", &t), "string writer")?;
    let end = core.find_in(writer, WRITER_MAX, WRITER_END)?.first_or("string writer end")? + END_LEN;
    let calls = core.find_in(writer, end - writer, &FTELL_CALL.replace("{t}", &t))?;
    ensure!(calls.len() == FTELL_CALLS, "string writer: expected {FTELL_CALLS} ftell calls, found {}", calls.len());
    let loop_calls = core.calls_in(save, SAVE_LEN, writer)?;
    let [a, b] = loop_calls.as_slice()[..] else {
        bail!("save function: expected 2 calls to string writer, found {}", loop_calls.len());
    };

    FTELL.set(core.live_ptr(slot)?);
    LOOP_RET_A.set(a + 5);
    LOOP_RET_B.set(b + 5);
    SAVE.set(core.hook(save, ENTRY, save_entry as unsafe extern "C" fn())?);
    WRITER.set(core.hook(writer, ENTRY, writer_entry as unsafe extern "C" fn())?);
    let hook = pick(cached_ftell::<true> as FtellFn, cached_ftell::<false> as FtellFn);
    for &call in calls.iter() {
        core.call_iat_to(call + 1, slot, hook)?;
    }
    Ok("cached in the string writer".into())
}

pub(super) fn stdio_stats() {
    if crate::base::driver::verify() {
        eprintln!("dmfast: stdio thread check calls={} other_thread={}", verify::CALLS, verify::OTHER_THREAD);
    }
}

pub(super) fn ftell_stats() {
    if crate::base::driver::verify() {
        eprintln!("dmfast: ftell cache checks={} mismatches={}", verify::CHECKS, verify::MISMATCHES);
    }
}
