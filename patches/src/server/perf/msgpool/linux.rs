//! `msgpool`, Linux: the pool constructor is hooked by [`construct`], the pool release by [`release`], and the array
//! destroy by [`destroy`].

use super::{BUILT, REFS, SIZE, SLOTS, adopt, keep, link, take};
use crate::base::code::{Cond, Foreign, read, write};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The pool constructor (cdecl: pool, count): references +8 = 1, `new[]` of count * 43Ch + 4 (call at +30h), each
/// message constructed inline (the qword at +4 from the constant at +3Eh), array +0, free head +4 = 0, then each
/// message's pool +42Ch, index +430h, next index +434h, and taken byte +438h = 0; the last one's next = -1.
const POOL_CTOR: &str = "55 B8 FF FF FF FF 57 56 53 83 EC 0C 8B 5C 24 20 8B 74 24 24 C7 43 08 01 00 00 00 81 FE 91 3A 1E 00 \
     77 09 69 C6 3C 04 00 00 83 C0 04 83 EC 0C 50 E8 ?? ?? ?? ?? 89 F2 83 C4 10 F3 0F 7E 05 ?? ?? ?? ?? 8D 68 04 89 30 \
     89 C7 89 E8 83 EA 01 78 61 31 C9 83 EA 01 C7 40 10 00 00 00 00 66 89 88 22 04 00 00 31 C9 66 89 48 02 B9 40 00 00 \
     00 66 89 08 B9 04 00 00 00 66 89 88 20 04 00 00 8D 48 18 05 3C 04 00 00 C7 40 E8 00 00 00 00 C7 40 EC 00 00 00 00 \
     C7 40 E0 00 00 00 00 89 88 D0 FB FF FF 66 0F D6 80 C8 FB FF FF C6 40 DF 00 83 FA FF 75 9F 89 2B C7 43 04 00 00 00 \
     00 85 F6 7E 73 89 9F 30 04 00 00 C7 87 34 04 00 00 00 00 00 00 C7 87 38 04 00 00 01 00 00 00 C6 87 3C 04 00 00 00 \
     83 FE 01 74 3E B8 01 00 00 00 8D B4 26 00 00 00 00 69 D0 3C 04 00 00 8B 0B 89 9C 11 2C 04 00 00 8B 0B 89 84 11 30 \
     04 00 00 8B 0B 83 C0 01 89 84 11 34 04 00 00 8B 0B C6 84 11 38 04 00 00 00 39 F0 75 CE 69 F6 3C 04 00 00 03 33 C7 \
     46 F8 FF FF FF FF";
/// Its first instructions, moved by the hook: push ebp ; mov eax,-1.
const CTOR_ENTRY: usize = 6;
/// Where the constructor reads the message's +4 and +8 (0 and 400h).
const CTOR_CONST: usize = 0x3E;
const CTOR_CONST_VALUE: u64 = 0x400 << 32;
/// The pool release (cdecl: pool): references -1; at 0, its array destroyed inline when there is one (`je` at +2Ah
/// when none), and the pool deleted.
const RELEASE: &str = "55 57 56 53 83 EC 1C 8B 44 24 30 8B 54 24 30 8B 40 08 89 44 24 04 83 E8 01 89 42 08 74 08 83 C4 1C 5B \
     5E 5F 5D C3 8B 12 85 D2 0F 84 ?? ?? ?? ?? 69 42 FC 3C 04 00 00";
const RELEASE_NONE_JE: usize = 0x2A;
/// Without an array: only the pool's sized delete.
const RELEASE_NONE: &str = "83 EC 08 6A 0C FF 74 24 3C E8 ?? ?? ?? ?? 83 C4 10 83 C4 1C 5B 5E 5F 5D C3";
/// The array destroy (cdecl: pool): each message destroyed and the array freed; nothing without one (`je` at +0Fh).
const DESTROY: &str = "55 57 56 53 83 EC 1C 8B 44 24 30 8B 10 85 D2 0F 84 ?? ?? ?? ?? 69 42 FC 3C 04 00 00 8D 0C 02 89 4C 24 \
     04 39 CA 0F 84";
const DESTROY_NONE_JE: usize = 0x0F;
const RETURN: &str = "83 C4 1C 5B 5E 5F 5D C3";
/// A message's release, at its pool's last reference: references -1, at 0 the array destroy (call at +0Eh), then the
/// pool's sized delete (tail call).
const MSG_RELEASE: &str =
    "83 6E 08 01 0F 85 ?? ?? ?? ?? 83 EC 0C 56 E8 ?? ?? ?? ?? 89 74 24 30 C7 44 24 34 0C 00 00 00";
const MSG_RELEASE_CALL: usize = 0x0E;
/// The entries of the release and the destroy, moved by their hooks: push ebp ; push edi ; push esi ; push ebx ;
/// sub esp,1Ch.
const ENTRY: usize = 7;

type PoolFn = unsafe extern "C" fn(usize);
type CtorFn = unsafe extern "C" fn(usize, u32);

static ORIGINAL_CTOR: Foreign<CtorFn> = Foreign::new();
static ORIGINAL_RELEASE: Foreign<PoolFn> = Foreign::new();
static ORIGINAL_DESTROY: Foreign<PoolFn> = Foreign::new();

/// Whether message `m` holds nothing to free (its buffer +10h and chained messages +424h, +428h): then its
/// destructor does nothing.
///
/// # Safety
/// `m` is a message.
unsafe fn empty(m: usize) -> bool {
    // SAFETY: a message (caller).
    unsafe { read::<usize>(m + 0x10) == 0 && read::<usize>(m + 0x424) == 0 && read::<usize>(m + 0x428) == 0 }
}

/// Constructs message `m` as the pool constructor's inline code does.
///
/// # Safety
/// `m` is a message only this thread uses, with nothing to free.
unsafe fn construct_message(m: usize) {
    // SAFETY: as above.
    unsafe {
        write(m, 0x40u32);
        write(m + 0x04, 0u32);
        write(m + 0x08, 0x400u32);
        write(m + 0x0C, m + 0x18);
        write(m + 0x10, 0u32);
        write(m + 0x41B, 0u8);
        write(m + 0x41C, 0u32);
        write(m + 0x420, 4u32);
        write(m + 0x424, 0u32);
        write(m + 0x428, 0u32);
    }
}

/// The pool constructor: a kept array when there is one (each message, holding nothing to free, constructed again
/// and linked), else the original. A message still holding something (not seen) sends the array to BYOND's destroy.
///
/// # Safety
/// As the constructor: `pool` is a pool being constructed.
unsafe extern "C" fn construct(pool: usize, count: u32) {
    if count as usize == SLOTS
        && let Some(array) = take()
    {
        // SAFETY: a kept array of SLOTS messages only this thread uses.
        unsafe {
            if (0..SLOTS).all(|i| empty(array + i * SIZE)) {
                for i in 0..SLOTS {
                    let m = array + i * SIZE;
                    construct_message(m);
                    link(m, pool, i);
                }
                return adopt(pool, array);
            }
            let detached = [array, 0, 0];
            ORIGINAL_DESTROY.get()(detached.as_ptr() as usize);
        }
    }
    BUILT.bump();
    // SAFETY: the original constructor with the same arguments.
    unsafe { ORIGINAL_CTOR.get()(pool, count) }
}

/// The pool release: at the last reference, [`keep`] first.
///
/// # Safety
/// As the release: `pool` is a live pool.
unsafe extern "C" fn release(pool: usize) {
    // SAFETY: a live pool (caller); at 1 reference, the caller's is the last.
    unsafe {
        if read::<u32>(pool + REFS) == 1 {
            keep(pool);
        }
        ORIGINAL_RELEASE.get()(pool)
    }
}

/// The array destroy (its pool's last reference gone): [`keep`] first.
///
/// # Safety
/// As the destroy: `pool` is a pool nothing refers to.
unsafe extern "C" fn destroy(pool: usize) {
    // SAFETY: as above.
    unsafe {
        keep(pool);
        ORIGINAL_DESTROY.get()(pool)
    }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let ctor = core.find_unique(POOL_CTOR, "message pool constructor")?;
    let constant = core.ptr_at(ctor + CTOR_CONST)?;
    ensure!(
        u64::from_le_bytes(core.array(constant)?) == CTOR_CONST_VALUE,
        "message pool constructor: unexpected message fields"
    );
    let rel = core.find_unique(RELEASE, "message pool release")?;
    core.require(core.jcc_at(rel + RELEASE_NONE_JE, Cond::Equal)?.target, RELEASE_NONE, "message pool release")?;
    let des = core.find_unique(DESTROY, "message array destroy")?;
    core.require(core.jcc_at(des + DESTROY_NONE_JE, Cond::Equal)?.target, RETURN, "message array destroy")?;
    let m = core.find_unique(MSG_RELEASE, "message release")?;
    ensure!(core.call_target(m + MSG_RELEASE_CALL)? == des, "message release: not the array destroy");
    ORIGINAL_DESTROY.set(core.hook(des, ENTRY, destroy as PoolFn)?);
    ORIGINAL_RELEASE.set(core.hook(rel, ENTRY, release as PoolFn)?);
    ORIGINAL_CTOR.set(core.hook(ctor, CTOR_ENTRY, construct as CtorFn)?);
    Ok(format!("{}{}", core.report(&[ctor, rel, des]), if crate::base::driver::verify() { " (verify)" } else { "" }))
}
