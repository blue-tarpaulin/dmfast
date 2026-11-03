//! `dmbread`, Linux: libbyond's u16, keyed u16, u32, float, n-byte, and id-array readers replaced, and every `ftell`
//! answered from glibc's stream state.
//!
//! glibc answers `ftell` on a plain read stream with an `lseek`, because `_offset` is -1 until a seek. After one real
//! `ftell`, [`tell`] stores what an `fseek` would (`_offset` = position + unread bytes), which glibc keeps current on
//! every refill, and answers from it. [`self_test`] checks this on a real file first.

use std::ffi::{c_int, c_long};

use libc::FILE;

use super::{
    BytesFn, ID_ARRAY, IdArray, IdsFn, KeyedFn, ReadFn, Stream, read_id_array, read_keyed, read_u16_keyed, read_value,
    result_unused, verify,
};
use crate::base::code::{Foreign, MallocFn};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::linux::alloc;
use crate::base::linux::glibc::{self, File, IO_CURRENTLY_PUTTING, IO_IS_APPENDING};
use crate::base::sync::Owned;
use crate::base::{Result, ensure};

type TellFn = unsafe extern "C" fn(*mut File) -> c_long;

/// Reads served from the stream's buffer, and the rest (through `fread`).
struct State {
    direct: u64,
    refill: u64,
}

static STATE: Owned<State> = Owned::new();

unsafe extern "C" {
    static stdin: *mut File;
    static stdout: *mut File;
    static stderr: *mut File;
    fn ftell(fp: *mut File) -> c_long;
}

/// Whether `fp` is a standard stream, which other threads use too.
#[inline(always)]
fn standard(fp: *mut File) -> bool {
    // SAFETY: glibc's standard stream pointers, only compared.
    unsafe { fp == stdin || fp == stdout || fp == stderr }
}

struct Glibc;

impl Stream for Glibc {
    /// On the owning thread, bytes in a plain read stream's buffer are copied out unlocked. Stock reads a value as
    /// separate locked `fgetc` calls, so sharing a stream across threads is already unsafe mid-value.
    unsafe fn read(fp: usize, buf: *mut u8, n: usize) -> usize {
        let fp = fp as *mut File;
        // SAFETY: a live glibc stream and `n` bytes at `buf` (caller); a read stream holds read_ptr..read_end.
        unsafe {
            if let Some(st) = STATE.get()
                && !standard(fp)
            {
                let f = &mut *fp;
                if f.flags & IO_CURRENTLY_PUTTING == 0 && (f.read_end as usize).wrapping_sub(f.read_ptr as usize) >= n {
                    std::ptr::copy_nonoverlapping(f.read_ptr, buf, n);
                    f.read_ptr = f.read_ptr.add(n);
                    st.direct += 1;
                    return n;
                }
                st.refill += 1;
            }
            libc::fread(buf.cast(), 1, n, fp.cast::<FILE>())
        }
    }
}

/// Replaces the float reader: the same 32 bits as the 32-bit reader, returned in st(0).
///
/// # Safety
/// `fp` is a readable stream (BYOND passes it).
unsafe extern "C" fn read_f32(fp: usize) -> f32 {
    // SAFETY: a readable stream (caller).
    f32::from_bits(unsafe { read_value::<Glibc, 4>(fp) })
}

/// Replaces every `ftell` call: [`tell`], checked in verify mode.
///
/// # Safety
/// `fp` is a live stream (BYOND passes it).
unsafe extern "C" fn fast_ftell(fp: *mut File) -> c_long {
    // SAFETY: a live stream (caller).
    unsafe { tell(fp, true) }
}

/// A plain read stream's position from `_offset` minus the unread bytes, once a real `ftell` has set `_offset`;
/// other streams' from `ftell`. `check`: verify mode applies.
///
/// # Safety
/// `fp` is a live stream.
unsafe fn tell(fp: *mut File, check: bool) -> c_long {
    // SAFETY: a live glibc stream (caller).
    unsafe {
        let f = &mut *fp;
        let plain = !standard(fp) && f.flags & (IO_CURRENTLY_PUTTING | IO_IS_APPENDING) == 0 && !f.read_base.is_null();
        if !plain {
            return ftell(fp);
        }
        let unread = f.read_end.offset_from(f.read_ptr) as i64;
        if f.offset < 0 {
            let real = ftell(fp);
            if real >= 0 {
                f.offset = real as i64 + unread;
            }
            return real;
        }
        let Ok(pos) = c_long::try_from(f.offset - unread) else { return ftell(fp) };
        if check && verify::disagrees(pos, || ftell(fp)) {
            f.offset = -1; // unknown: glibc asks the kernel again
            return ftell(fp);
        }
        pos
    }
}

/// Checks on the program's own file that glibc keeps `_offset` right across refills, large reads, and seeks.
fn self_test() -> Result<()> {
    // SAFETY: a stream this function opens, reads, and closes.
    unsafe {
        let fp = libc::fopen(c"/proc/self/exe".as_ptr(), c"rb".as_ptr());
        ensure!(!fp.is_null(), "cannot open /proc/self/exe to test ftell");
        let f = fp.cast::<File>();
        let mut buf = vec![0u8; 40000];
        let agrees = || {
            let real = ftell(f);
            real >= 0 && real == tell(f, false)
        };
        let mut reads = |sizes: &[usize]| {
            sizes.iter().all(|&n| libc::fread(buf.as_mut_ptr().cast(), 1, n, fp) == n && agrees() && agrees())
        };
        let ok = glibc::getc(f) >= 0
            && agrees()
            && reads(&[1, 2, 4000, 7, 4096, 9000, 3, 40000, 5, 100])
            && libc::fseek(fp, 12345, libc::SEEK_SET) == 0
            && agrees()
            && reads(&[3, 5000, 1])
            && libc::fseek(fp, 777, libc::SEEK_CUR) == 0
            && agrees()
            && reads(&[1, 4100]);
        libc::fclose(fp);
        ensure!(ok, "glibc does not keep FILE._offset current on reads");
        Ok(())
    }
}

/// A helper that reads byte by byte: its signature, and where it calls the stdio function it replaces.
struct Helper {
    /// Its part's name.
    name: &'static str,
    ida: &'static str,
    reads: &'static [usize],
}

/// push esi ; push ebx ; sub esp,10h ; mov esi,[esp+1Ch] ; push esi ; call fgetc ; mov [esp],esi ; mov ebx,eax ;
/// call fgetc ; add esp,14h ; shl eax,8 ; or eax,ebx ; pop ebx ; pop esi ; ret
const U16: Helper = Helper {
    name: "u16 reader",
    ida: "56 53 83 EC 10 8B 74 24 1C 56 E8 ?? ?? ?? ?? 89 34 24 89 C3 E8 ?? ?? ?? ?? 83 C4 14 C1 E0 08 09 D8 5B 5E C3",
    reads: &[0x0A, 0x14],
};
/// push edi ; push esi ; push ebx ; mov edi,[esp+10h] ; mov ebx,[esp+14h] ; sub esp,0Ch ; push edi ; call fgetc ;
/// mov [esp],edi ; mov esi,eax ; call fgetc ; add esp,10h ; shl eax,8 ; or eax,esi ; xor eax,ebx ; pop ebx ; pop esi ;
/// pop edi ; ret
const U16_KEYED: Helper = Helper {
    name: "keyed u16 reader",
    ida: "57 56 53 8B 7C 24 10 8B 5C 24 14 83 EC 0C 57 E8 ?? ?? ?? ?? 89 3C 24 89 C6 E8 ?? ?? ?? ?? 83 C4 10 \
          C1 E0 08 09 F0 31 D8 5B 5E 5F C3",
    reads: &[0x0F, 0x19],
};
/// Four fgetc calls combined with shl/or.
const U32: Helper = Helper {
    name: "u32 reader",
    ida: "57 56 53 8B 74 24 10 83 EC 0C 56 E8 ?? ?? ?? ?? 89 34 24 89 C7 E8 ?? ?? ?? ?? 89 34 24 89 C3 E8 ?? ?? ?? ?? \
          C1 E3 08 89 34 24 C1 E0 10 09 C3 E8 ?? ?? ?? ?? 09 FB 83 C4 10 C1 E0 18 09 D8 5B 5E 5F C3",
    reads: &[0x0B, 0x15, 0x1F, 0x2F],
};
/// The same four reads, then fld.
const F32: Helper = Helper {
    name: "float reader",
    ida: "57 56 53 83 EC 1C 8B 74 24 2C 56 E8 ?? ?? ?? ?? 89 34 24 89 C7 E8 ?? ?? ?? ?? 89 34 24 89 C3 E8 ?? ?? ?? ?? \
          C1 E3 08 89 34 24 C1 E0 10 09 C3 E8 ?? ?? ?? ?? 09 FB C1 E0 18 09 D8 89 44 24 1C D9 44 24 1C 83 C4 20 \
          5B 5E 5F C3",
    reads: &[0x0B, 0x15, 0x1F, 0x2F],
};
/// n x (getc ^ key, key += 9).
const KEYED: Helper = Helper {
    name: "byte reader",
    ida: "55 57 56 53 83 EC 0C 8B 74 24 24 8B 7C 24 28 0F B6 6C 24 2C 85 F6 74 23 8B 5C 24 20 01 DE 66 90 \
          83 EC 0C 83 C3 01 57 E8 ?? ?? ?? ?? 83 C4 10 31 E8 83 C5 09 88 43 FF 39 F3 75 E5 83 C4 0C 5B 5E 5F 5D C3",
    reads: &[0x27],
};

/// The helper, found by its signature, if its reads call `stdio`.
fn find(core: &Image, h: &Helper, stdio: usize) -> Result<usize> {
    let f = core.find_unique(h.ida, &format!(".dmb {}", h.name))?;
    for &at in h.reads {
        ensure!(core.call_target(f + at)? == stdio, ".dmb {}: unexpected read call", h.name);
    }
    Ok(f)
}

/// The n-byte reader, if every caller ignores its result.
fn keyed(core: &Image) -> Result<()> {
    let f = find(core, &KEYED, glibc::getc as unsafe extern "C" fn(*mut File) -> c_int as usize)?;
    // Known callers continue with `pop eax ; pop edx ; push -1`, `mov [esp],esi ; call ..`, or
    // `mov eax,[ebp+0] ; mov byte [eax+ebx],0`.
    result_unused(core, f, |next| {
        Ok(core.is_at(next, "58 5A 6A FF")?
            || core.is_at(next, "89 34 24 E8")?
            || core.is_at(next, "8B 45 00 C6 04 18 00")?)
    })?;
    core.jump(f, 5, read_keyed::<Glibc> as BytesFn)
}

/// The id-array reader: count via u16, malloc(count*4), wide flag ? u32 reads : u16 reads.
const IDS: &str = "55 57 56 53 83 EC 18 8B 7C 24 30 8B 5C 24 2C 57 E8 ?? ?? ?? ?? 66 89 03 89 C5 0F B7 C0 C1 E0 02 89 04 24 \
                   E8 ?? ?? ?? ?? 83 C4 10 80 3D ?? ?? ?? ?? 00 89 43 08 75 39 31 F6 66 85 ED 74 22 66 90 83 EC 0C 57 \
                   E8 ?? ?? ?? ?? 8B 53 08 0F B7 CE 83 C4 10 0F B7 C0 83 C6 01 89 04 8A 66 39 33 77 E0 83 C4 0C 5B 5E 5F 5D C3 \
                   8D B4 26 00 00 00 00 90 66 85 ED 74 EB 31 F6 EB 0A 8D B4 26 00 00 00 00 8B 43 08 83 EC 0C 0F B7 D6 \
                   83 C6 01 57 8D 2C 90 E8 ?? ?? ?? ?? 83 C4 10 89 45 00 66 39 33 77 E0 83 C4 0C 5B 5E 5F 5D C3";
/// In [`IDS`]: the count's read, the `malloc` call, the format flag, the narrow and the wide ids' reads.
const COUNT_READ: usize = 0x10;
const MALLOC_CALL: usize = 0x23;
const WIDE_FLAG: usize = 0x2D;
const NARROW_READ: usize = 0x44;
const WIDE_READ: usize = 0x90;

/// The id-array reader, if its value readers are the u16 and u32 readers.
fn id_array(cx: &Ctx, u16_fn: &Result<usize>, u32_fn: &Result<usize>) -> Result<()> {
    let core = cx.core();
    let f = core.find_unique(IDS, ".dmb id array reader")?;
    let (u16_fn, u32_fn) = (u16_fn.clone()?, u32_fn.clone()?);
    let call = |at: usize| core.call_target(f + at);
    ensure!(
        call(COUNT_READ)? == u16_fn && call(NARROW_READ)? == u16_fn && call(WIDE_READ)? == u32_fn,
        ".dmb id array reader: unexpected value readers"
    );
    let malloc = alloc::malloc as MallocFn;
    ensure!(
        core.import_calls("malloc", malloc)?.contains(&(f + MALLOC_CALL)),
        ".dmb id array reader: does not allocate with malloc"
    );
    ID_ARRAY.commit(cx, IdArray { malloc: Foreign::at(malloc), wide: core.ptr_at(f + WIDE_FLAG)? });
    core.jump(f, 5, read_id_array::<Glibc> as IdsFn)
}

/// Every `ftell` call becomes one of [`fast_ftell`].
fn ftells(core: &Image) -> Result<()> {
    let sites = core.import_calls("ftell", ftell as TellFn)?;
    ensure!(!sites.is_empty(), "no ftell call sites");
    sites.iter().try_for_each(|&s| core.retarget(s + 1, fast_ftell as TellFn))
}

/// Each reader is a part: one that doesn't match is left out. All need glibc to keep `FILE._offset` current
/// ([`self_test`]).
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    self_test()?;
    let fgetc = libc::fgetc as unsafe extern "C" fn(*mut FILE) -> c_int as usize;
    let [u16_fn, u16k_fn, u32_fn, f32_fn] = [&U16, &U16_KEYED, &U32, &F32].map(|h| find(core, h, fgetc));
    let mut parts = cx.parts();
    for (h, f, hook) in [
        (&U16, &u16_fn, read_value::<Glibc, 2> as ReadFn as usize),
        (&U16_KEYED, &u16k_fn, read_u16_keyed::<Glibc> as KeyedFn as usize),
        (&U32, &u32_fn, read_value::<Glibc, 4> as ReadFn as usize),
        (&F32, &f32_fn, read_f32 as unsafe extern "C" fn(usize) -> f32 as usize),
    ] {
        parts.named(h.name, || core.jump(f.clone()?, 5, hook));
    }
    parts.named(KEYED.name, || keyed(core));
    parts.named("id array reader", || id_array(cx, &u16_fn, &u32_fn));
    parts.named("ftell", || ftells(core));
    let report = parts.finish()?;
    STATE.set(State { direct: 0, refill: 0 })?;
    Ok(report)
}

pub(super) fn print_stats() {
    // SAFETY: exit-time stats: the server no longer reads .dmb files.
    if let Some(st) = unsafe { STATE.shared() } {
        eprintln!(
            "dmfast: dmb reader buffered reads={} refills={} ftell mismatches={} (verify={})",
            st.direct,
            st.refill,
            verify::MISMATCHES,
            crate::base::driver::verify()
        );
    }
}
