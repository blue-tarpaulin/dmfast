//! `dmbread`, Windows: byondcore's u32, keyed u16, n-byte, and id-array readers replaced, and the string reader's
//! `ftell` answered from the position the readers keep.
//!
//! The string reader asks `ftell` (a syscall) before each length and string, its obfuscation being keyed by position.
//! Once a real `ftell` gives a stream's position, reads through the replaced helpers keep it current; the string
//! table loop, the reader's only caller, reads only through them in between. An answer is given only while the UCRT
//! `FILE`'s `_ptr` and `_cnt` are as the last tracked read left them.

use std::ffi::c_void;

use super::{
    BytesFn, ID_ARRAY, IdArray, IdsFn, KeyedFn, ReadFn, Stream, read_id_array, read_keyed, read_u16_keyed, read_value,
    result_unused, verify,
};
use crate::base::code::{Cond, Foreign, Stub, hx, live_u32, write};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::{Installed, Owned};
use crate::base::windows::ucrt::ucrt_file_layout;
use crate::base::{Result, ensure};

type FreadFn = unsafe extern "C" fn(*mut c_void, usize, usize, usize) -> usize;
type FtellFn = unsafe extern "C" fn(usize) -> i32;

/// UCRT `FILE` fields ([`ucrt_file_layout`] checks them): the next byte, the bytes left in the buffer, the flags.
const PTR: usize = 0;
const CNT: usize = 8;
const FLAGS: usize = 0xC;
/// `_flags` bits: open for reading, open for writing.
const IOREAD: u32 = 0x1;
const IOWRITE: u32 = 0x2;

/// ucrtbase's `fread` and `ftell`, which byondcore calls.
struct Crt {
    fread: Foreign<FreadFn>,
    ftell: Foreign<FtellFn>,
}

static CRT: Installed<Crt> = Installed::new();
static STATE: Owned<State> = Owned::new();

struct State {
    /// The stream whose position is known.
    tracked: Option<Tracked>,
    /// `ftell` calls answered from `tracked`, and passed to the CRT.
    hits: u64,
    misses: u64,
    /// Reads served from the stream's buffer.
    direct: u64,
}

/// A stream at position `pos`, its `_ptr` and `_cnt` as they were there.
struct Tracked {
    fp: usize,
    pos: i32,
    ptr: u32,
    cnt: u32,
}

impl Tracked {
    /// `fp`, a live UCRT `FILE`, now at `pos`.
    unsafe fn at(fp: usize, pos: i32) -> Tracked {
        // SAFETY: a live `FILE` (caller).
        unsafe { Tracked { fp, pos, ptr: live_u32(fp + PTR), cnt: live_u32(fp + CNT) } }
    }

    /// Whether `fp` is this stream (then live), still at `pos`.
    unsafe fn holds(&self, fp: usize) -> bool {
        // SAFETY: this live `FILE` (caller).
        self.fp == fp && unsafe { live_u32(fp + PTR) == self.ptr && live_u32(fp + CNT) == self.cnt }
    }
}

/// # Safety
/// `fp` is a stream `ftell` accepts.
unsafe fn ftell(fp: usize) -> i32 {
    // SAFETY: ucrtbase's `ftell` (install) on the caller's stream.
    unsafe { CRT.get().ftell.get()(fp) }
}

struct Ucrt;

impl Stream for Ucrt {
    /// On the owning thread, bytes already in a read-only stream's buffer are taken as `_fgetc_nolock` does (`_ptr`
    /// forward, `_cnt` down); stock reads with separately locked `fgetc` calls, so any sharing that works in stock is
    /// already serialised by BYOND. A tracked stream's position follows.
    unsafe fn read(fp: usize, buf: *mut u8, n: usize) -> usize {
        // SAFETY: `fp` is BYOND's stream and `buf` holds `n` bytes (caller); a read-only UCRT `FILE` has `_cnt` bytes
        // at `_ptr`; ucrtbase's `fread` (install).
        unsafe {
            let fread = || CRT.get().fread.get()(buf.cast(), 1, n, fp);
            let Some(st) = STATE.get() else { return fread() };
            let held = st.tracked.as_ref().is_some_and(|t| t.holds(fp));
            let got = if fp != 0
                && live_u32(fp + CNT) as i32 >= n as i32
                && live_u32(fp + FLAGS) & (IOREAD | IOWRITE) == IOREAD
            {
                let (p, cnt) = (live_u32(fp + PTR) as usize, live_u32(fp + CNT));
                std::ptr::copy_nonoverlapping(p as *const u8, buf, n);
                write(fp + PTR, (p + n) as u32);
                write(fp + CNT, cnt - n as u32);
                st.direct += 1;
                n
            } else {
                fread()
            };
            if let Some(t) = &st.tracked
                && t.fp == fp
            {
                // Otherwise an untracked access in between, the end of the file, or an error: the CRT knows best.
                st.tracked = (held && got == n).then(|| Tracked::at(fp, t.pos + n as i32));
            }
            got
        }
    }
}

/// Replaces the string reader's `ftell`: the tracked position, else the CRT's answer, which starts tracking.
///
/// # Safety
/// `fp` is a stream (BYOND passes it).
unsafe extern "C" fn cached_ftell(fp: usize) -> i32 {
    // SAFETY: a live UCRT `FILE` (caller) when nonzero.
    unsafe {
        let Some(st) = STATE.get() else { return ftell(fp) };
        if let Some(t) = &st.tracked
            && t.holds(fp)
        {
            let pos = t.pos;
            st.hits += 1;
            if verify::disagrees(pos, || ftell(fp)) {
                st.tracked = None;
                return ftell(fp);
            }
            return pos;
        }
        st.misses += 1;
        let pos = ftell(fp);
        st.tracked = (pos >= 0 && fp != 0).then(|| Tracked::at(fp, pos));
        pos
    }
}

/// The 32-bit reader: push ebp ; mov ebp,esp ; push ebx ; push esi ; mov esi,[fgetc] ; push edi ; mov edi,[ebp+8] ;
/// 4x (push edi ; call esi) combined with shl/or ; ... ; ret
fn u32_reader(fgetc: &str) -> String {
    format!(
        "55 8B EC 53 56 8B 35 {fgetc} 57 8B 7D 08 57 FF D6 57 8B D8 FF D6 C1 E0 08 57 0B D8 FF D6 \
         C1 E0 10 57 0B D8 FF D6 83 C4 10 C1 E0 18 0B C3 5F 5E 5B 5D C3"
    )
}

/// Where the string reader calls the keyed 16-bit reader.
const U16_CALL: usize = 0x27;
/// The string reader after its `ftell`, up to its call of the n-byte reader: push edi ; call [ebp+14h] (ftell) ;
/// sub al,[ebp-4] ; movzx eax,al ; push eax ; push edi ; push esi ; push [ebx] ; call
const BYTES_CALL: &str = "57 FF 55 14 2A 45 FC 0F B6 C0 50 57 56 FF 33 E8";
/// Where its `call` is.
const BYTES_CALL_AT: usize = 15;
/// How far into the string reader to look for [`BYTES_CALL`].
const READER_LEN: usize = 0xA0;
/// The code after a call of the n-byte reader that checks its length: add esp,10h ; cmp ebx,100h ;
/// jae <a call that never returns> ; lea eax,...
const RANGE_CHECKED: &str = "83 C4 10 81 FB 00 01 00 00 0F 83 ?? ?? ?? ?? 8D 85";
/// Where its `jae` is.
const RANGE_JAE: usize = 9;

/// The string reader: its keyed 16-bit and n-byte helpers, and its `ftell` through the cache.
fn string_reader(core: &Image, fgetc: &str, ftell_slot: usize) -> Result<()> {
    let getc = hx(core.iat_slot("getc")?);
    // push ebp ; mov ebp,esp ; push ecx ; mov eax,[ftell] ; push ebx ; mov ebx,[ebp+14h] ; push esi ; push edi ;
    // mov edi,[ebp+0Ch] ; xor esi,esi ; mov [ebp-4],ebx ; mov [ebp+14h],eax ; (nop) ; loop: push edi ; call eax ;
    // sub eax,ebx ; push eax ; push edi ; call read_u16_keyed
    let reader = core.find_unique(
        &format!(
            "55 8B EC 51 A1 {} 53 8B 5D 14 56 57 8B 7D 0C 33 F6 89 5D FC 89 45 14 ?? ?? ?? ?? ?? ?? \
             57 FF D0 2B C3 50 57 E8",
            hx(ftell_slot)
        ),
        ".dmb string reader",
    )?;
    // keyed u16: push ebp ; mov ebp,esp ; push ebx ; push [ebp+8] ; call [fgetc] ; push [ebp+8] ; mov ebx,eax ;
    // call [fgetc] ; add esp,8 ; shl eax,8 ; or eax,ebx ; xor eax,[ebp+0Ch] ; pop ebx ; pop ebp ; ret
    let u16_fn = core.follow_call(
        reader + U16_CALL,
        &format!(
            "55 8B EC 53 FF 75 08 FF 15 {fgetc} FF 75 08 8B D8 FF 15 {fgetc} 83 C4 08 C1 E0 08 0B C3 33 45 0C 5B 5D C3"
        ),
        ".dmb u16 reader",
    )?;
    let call = core.find_unique_in(reader, READER_LEN, BYTES_CALL, ".dmb string reader: byte-read call")?;
    // n-byte: push ebp ; mov ebp,esp ; push esi ; push edi ; mov edi,[ebp+0Ch] ; xor esi,esi ; test edi,edi ;
    // je done ; push ebx ; mov bl,[ebp+14h] ; loop: push [ebp+10h] ; call [getc] ; mov ecx,[ebp+8] ; xor al,bl ;
    // add esp,4 ; add bl,9 ; mov [esi+ecx],al ; inc esi ; cmp esi,edi ; jb loop ; pop ebx ; done: pop edi ;
    // pop esi ; pop ebp ; ret
    let keyed_fn = core.follow_call(
        call + BYTES_CALL_AT,
        &format!(
            "55 8B EC 56 57 8B 7D 0C 33 F6 85 FF 74 21 53 8A 5D 14 FF 75 10 FF 15 {getc} 8B 4D 08 32 C3 \
             83 C4 04 80 C3 09 88 04 0E 46 3B F7 72 E4 5B 5F 5E 5D C3"
        ),
        ".dmb byte reader",
    )?;
    // The cache is exact because the string table loop is the reader's only caller (module docs).
    let callers = core.calls_to(reader)?.len();
    ensure!(callers == 1, ".dmb string reader: {callers} callers, expected only the string table loop");
    // Known callers continue with the range check, `push ebx ; call [..]`, or `mov eax,[ebx]`.
    let range_checked = |next: usize| -> Result<bool> {
        Ok(core.is_at(next, RANGE_CHECKED)?
            && core.is_at(core.jcc_at(next + RANGE_JAE, Cond::AboveOrEqual)?.target, "E8 ?? ?? ?? ?? CC")?)
    };
    result_unused(core, keyed_fn, |next| {
        Ok(range_checked(next)? || core.is_at(next, "53 FF 15")? || core.is_at(next, "8B 03")?)
    })?;
    core.jump(u16_fn, 5, read_u16_keyed::<Ucrt> as KeyedFn)?;
    core.jump(keyed_fn, 5, read_keyed::<Ucrt> as BytesFn)?;
    // mov eax,[ftell] -> mov eax,cached_ftell
    core.write_stub(reader + 4, 5, Stub::new().bytes_u32(&[0xB8], cached_ftell as FtellFn as usize as u32, &[]))
}

/// The id-array reader: push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+0Ch] ; push esi ; push edi ; push ebx ;
/// call u16 ; mov edi,[ebp+8] ; mov [edi],ax ; movzx eax,ax ; shl eax,2 ; push eax ; call [malloc] ; mov [edi+8],eax ;
/// ... ; cmp byte [wide],cl ; je narrow ; wide: push ebx ; call u32 ; ... store ; narrow: push ebx ; call u16 ;
/// movzx edx,ax ; ... store
const IDS: &str = "55 8B EC 53 8B 5D 0C 56 57 53 E8 ?? ?? ?? ?? 8B 7D 08 66 89 07 0F B7 C0 C1 E0 02 50 FF 15 ?? ?? ?? ?? \
                   89 47 08 33 C9 0F B7 07 83 C4 08 33 F6 38 0D ?? ?? ?? ?? 74 26 66 3B C8 73 41 0F 1F 40 00 53 E8 ?? ?? \
                   ?? ?? 8B 4F 08 83 C4 04 0F B7 D6 46 89 04 91 66 3B 37 72 E8 5F 5E 5B 5D C3 66 3B C8 73 1B 53 E8 ?? ?? \
                   ?? ?? 0F B7 D0 83 C4 04 8B 47 08 0F B7 CE 46 89 14 88 66 3B 37 72 E5 5F 5E 5B 5D C3";
/// In [`IDS`]: the count's read, `malloc`'s import slot, the format flag, the wide and the narrow ids' reads.
const COUNT_READ: usize = 0x0A;
const MALLOC_SLOT: usize = 0x1E;
const WIDE_FLAG: usize = 0x31;
const WIDE_READ: usize = 0x41;
const NARROW_READ: usize = 0x63;

/// The id-array reader, with its value readers checked: the plain 16-bit one and the 32-bit one.
fn id_array(cx: &Ctx, fgetc: &str) -> Result<()> {
    let core = cx.core();
    let f = core.find_unique(IDS, ".dmb id array reader")?;
    // The plain 16-bit reader: (fgetc & 0xFFFF) | fgetc << 8.
    let u16_fn = core.follow_call(
        f + COUNT_READ,
        &format!("55 8B EC 53 FF 75 08 FF 15 {fgetc} FF 75 08 0F B7 D8 FF 15 {fgetc} 83 C4 08 C1 E0 08 0B C3 5B 5D C3"),
        ".dmb plain u16 reader",
    )?;
    ensure!(core.call_target(f + NARROW_READ)? == u16_fn, ".dmb id array reader: unexpected value readers");
    core.follow_call(f + WIDE_READ, &u32_reader(fgetc), ".dmb u32 reader")?;
    let malloc = core.iat_slot("malloc")?;
    ensure!(core.ptr_at(f + MALLOC_SLOT)? == malloc, ".dmb id array reader: does not allocate with malloc");
    ID_ARRAY.commit(cx, IdArray { malloc: Foreign::at(core.live_ptr(malloc)?), wide: core.ptr_at(f + WIDE_FLAG)? });
    core.jump(f, 5, read_id_array::<Ucrt> as IdsFn)
}

/// Each reader is a part: one that doesn't match is left out (the `ftell` cache checks the stream on every answer,
/// so any mix is right).
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let fgetc = hx(core.iat_slot("fgetc")?);
    // The buffer reads use the UCRT `FILE` layout: byondcore's streams must be ucrtbase's.
    ucrt_file_layout()?;
    let (fread_slot, ftell_slot) = (core.ucrt_slot("fread")?, core.ucrt_slot("ftell")?);
    let crt = Crt { fread: Foreign::at(core.live_ptr(fread_slot)?), ftell: Foreign::at(core.live_ptr(ftell_slot)?) };
    CRT.commit(cx, crt);
    let mut parts = cx.parts();
    parts.named("u32 reader", || {
        let f = core.find_unique(&u32_reader(&fgetc), ".dmb u32 reader")?;
        core.jump(f, 5, read_value::<Ucrt, 4> as ReadFn)
    });
    parts.named("string reader", || string_reader(core, &fgetc, ftell_slot));
    parts.named("id array reader", || id_array(cx, &fgetc));
    let report = parts.finish()?;
    STATE.set(State { tracked: None, hits: 0, misses: 0, direct: 0 })?;
    Ok(report)
}

pub(super) fn print_stats() {
    // SAFETY: exit-time stats: the server no longer reads .dmb files.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: dmb reader ftell hits={} misses={} verify={} mismatches={}; buffered reads={}",
            st.hits,
            st.misses,
            crate::base::driver::verify(),
            verify::MISMATCHES,
            st.direct
        );
    }
}
