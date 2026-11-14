//! `msgpool`, Windows: the pool constructor is hooked by [`construct`], and both inlined pool destroys first call
//! [`keep`] through a stub.

use super::{BUILT, SIZE, SLOTS, adopt, keep, link, take};
use crate::base::code::{Cond, Foreign, Stub};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The pool constructor (thiscall: ecx = pool, [esp+4] = count; returns the pool): references +8 = 1, `new[]` of
/// count * 43Ch + 4, the vector constructor with the message destructor (pushed at +6Ah) and constructor (+6Fh),
/// array +0, free head +4 = 0, then each message's pool +42Ch, index +430h, next index +434h, and taken byte +438h =
/// 0; the last one's next = -1.
const POOL_CTOR: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 51 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 \
     00 00 00 00 8B F1 8B 5D 08 33 C9 8B C3 C7 46 08 01 00 00 00 BA 3C 04 00 00 89 5D 08 F7 E2 0F 90 C1 F7 D9 0B C8 33 \
     C0 83 C1 04 0F 92 C0 F7 D8 0B C1 50 E8 ?? ?? ?? ?? 83 C4 04 89 45 F0 C7 45 FC 00 00 00 00 85 C0 74 6C 68 ?? ?? ?? \
     ?? 68 ?? ?? ?? ?? 53 8D 78 04 89 18 68 3C 04 00 00 57 E8 ?? ?? ?? ?? 89 3E 85 FF 74 51 33 D2 C7 46 04 00 00 00 00 \
     85 DB 7E 4B 33 C9 66 0F 1F 44 00 00 8B 06 8D 89 3C 04 00 00 89 74 01 F0 8B 06 89 54 01 F4 42 8B 06 89 54 01 F8 8B \
     06 C6 44 01 FC 00 3B D3 7C DC 8B 06 69 CA 3C 04 00 00 C7 44 01 F8 FF FF FF FF";
/// Its first instructions, moved by the hook: push ebp ; mov ebp,esp ; push -1.
const ENTRY: usize = 5;
const DTOR_PUSH: usize = 0x6A;
const CTOR_PUSH: usize = 0x6F;
/// The message constructor (thiscall): stores into the message only.
const MSG_CTOR: &str = "55 8B EC 51 8D 41 18 C7 81 28 04 00 00 00 00 00 00 89 41 0C 8B C1 C7 81 20 04 00 00 04 00 00 00 C7 01 \
     40 00 00 00 C7 41 10 00 00 00 00 C7 81 24 04 00 00 00 00 00 00 C7 41 04 00 00 00 00 C7 81 1C 04 00 00 00 00 00 00 \
     C7 41 08 00 04 00 00 89 4D FC C6 81 1B 04 00 00 00 8B E5 5D C3";
/// The message destructor the vector calls: a jump to the destructor (thiscall), which frees the message's buffer
/// (+10h) and its chained messages (+424h, +428h).
const MSG_DTOR: &str = "E9";
const MSG_DTOR_BODY: &str = "56 57 8B F9 8B 47 10 85 C0 74 ?? 50 FF 15 ?? ?? ?? ?? 83 C4 04 8B B7 24 04 00 00 85 F6 74";
/// A pool's destroy, inlined where a reference goes (edi = pool): `add [edi+8],-1 ; jne end` (replaced), then, for an
/// array (+8: `test eax,eax ; je`), the vector destructor with the message destructor (pushed at +0Ch) and the
/// array's free, and the pool's sized delete; `end` at +47h.
const DESTROY: &str = "83 47 08 FF 75 41 8B 07 85 C0 74 30 68 ?? ?? ?? ?? FF 70 FC 8D 70 FC C7 45 FC 00 00 00 00 68 3C 04 00 \
     00 50 E8 ?? ?? ?? ?? 69 06 3C 04 00 00 83 C0 04 50 56 E8 ?? ?? ?? ?? 83 C4 08 6A 0C 57 E8 ?? ?? ?? ?? 83 C4 08";
const DESTROY_DTOR_PUSH: usize = 0x0C;
const DESTROY_LEN: usize = 6;
const DESTROY_END: usize = 0x47;

type MsgFn = unsafe extern "thiscall" fn(usize);
type PoolFn = unsafe extern "thiscall" fn(usize, u32) -> usize;

static DTOR: Foreign<MsgFn> = Foreign::new();
static CTOR: Foreign<MsgFn> = Foreign::new();
static ORIGINAL: Foreign<PoolFn> = Foreign::new();

/// The pool constructor: a kept array when there is one (each message destroyed and constructed again by BYOND's
/// own functions, then linked), else the original.
///
/// # Safety
/// As the constructor: `pool` is a pool being constructed.
unsafe extern "thiscall" fn construct(pool: usize, count: u32) -> usize {
    if count as usize == SLOTS
        && let Some(array) = take()
    {
        // SAFETY: a kept array of SLOTS messages only this thread uses; BYOND's destructor and constructor.
        unsafe {
            for i in 0..SLOTS {
                let m = array + i * SIZE;
                DTOR.get()(m);
                CTOR.get()(m);
                link(m, pool, i);
            }
            adopt(pool, array);
        }
        return pool;
    }
    BUILT.bump();
    // SAFETY: the original constructor with the same arguments.
    unsafe { ORIGINAL.get()(pool, count) }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(POOL_CTOR, "message pool constructor")?;
    let dtor = core.ptr_at(f + DTOR_PUSH + 1)?;
    let ctor = core.ptr_at(f + CTOR_PUSH + 1)?;
    core.require(ctor, MSG_CTOR, "message constructor")?;
    core.require(dtor, MSG_DTOR, "message destructor")?;
    core.require(core.branch_at(dtor)?.target, MSG_DTOR_BODY, "message destructor")?;
    let sites = core.find_all(DESTROY)?;
    ensure!(sites.len() == 2, "message pool destroy: {} sites, expected 2", sites.len());
    for &at in sites.iter() {
        ensure!(core.ptr_at(at + DESTROY_DTOR_PUSH + 1)? == dtor, "message pool destroy: another destructor");
        core.untouched(&[(at, DESTROY_LEN)])?;
    }
    DTOR.set(dtor);
    CTOR.set(ctor);
    for &at in sites.iter() {
        let mut s = Stub::new();
        let last = s.label();
        s.bytes(&[0x83, 0x7F, 0x08, 0x01]) // cmp dword [edi+8],1
            .jcc8(Cond::NotEqual, last)
            .bytes(&[0x57]) // push edi
            .call(keep as unsafe extern "C" fn(usize))
            .bytes(&[0x83, 0xC4, 0x04]) // add esp,4
            .bind(last)
            .bytes(&[0x83, 0x47, 0x08, 0xFF]) // add dword [edi+8],-1
            .jcc(Cond::NotEqual, at + DESTROY_END)
            .jmp(at + DESTROY_LEN);
        core.jump(at, DESTROY_LEN, s.exec()?)?;
    }
    ORIGINAL.set(core.hook(f, ENTRY, construct as PoolFn)?);
    let mut all = vec![f];
    all.extend(sites.iter());
    Ok(format!("{}{}", core.report(&all), if crate::base::driver::verify() { " (verify)" } else { "" }))
}
