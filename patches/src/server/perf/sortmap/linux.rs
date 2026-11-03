//! `sortmap`, Linux: libbyond's sorted map insert and merge replaced by the shared ones.

use std::arch::naked_asm;
use std::ffi::c_void;

use super::{ALLOC, Alloc, MERGE, Map, Merge, insert, merge_into};
use crate::base::code::{Addr, Foreign, live_u32};
use crate::base::feature::Ctx;
use crate::base::linux::{alloc, symbol};
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

/// The insert: push ebp/edi/esi/ebx ; sub esp,1Ch ; flag, key, map from the stack ; `movzx ebp,word [eax+4] ;
/// mov ecx,[eax]` ; binary search over 12-byte entries ; (miss) `lea eax,[ebp+ebp*2+3]` ; ... realloc ; (failed)
/// `mov word [eax+4],0` ; call out_of_memory
const INSERT: &str = "55 57 56 53 83 EC 1C 8B 44 24 38 8B 7C 24 34 89 44 24 0C 8B 44 24 30 0F B7 68 04 8B 08 66 85 ED \
                      0F 84 ?? ?? ?? ?? 89 EA 31 F6 EB ?? 8D 74 26 00 8D 73 01 66 39 F2 76 ?? 0F B7 DA 0F B7 C6 01 D8 \
                      89 C3 25 FE FF 01 00 D1 FB 01 D8 8D 04 81 39 38 72 ?? 0F 86 ?? ?? ?? ?? 89 DA 66 39 F2 77 ?? \
                      8D 44 6D 03";
/// The insert's out-of-memory path: mov word [eax+4],0 ; call out_of_memory.
const INSERT_FAIL: usize = 0x103;

/// The whole merge: for each source entry with values, advance the destination index past smaller keys, insert the
/// key if absent, then share the source array (FreeShared ; CopyShared) or join both into a new one (AllocShared ;
/// memcpy ; memcpy ; FreeShared) and link the last old value's proc (inlined lookup ; cmp [eax+28h],0FFFFh) to the
/// first appended one. Out of memory: tail jump to the handler.
const MERGE_FN: &str = "55 89 D5 31 C9 57 56 53 83 EC 1C 0F B7 52 04 89 44 24 08 66 89 4C 24 0E 66 85 D2 0F 84 31 01 00 00 \
     89 6C 24 04 31 FF 8D B4 26 00 00 00 00 66 90 0F B7 C7 8D 0C 40 8B 44 24 04 8B 00 8D 34 88 0F B7 46 \
     08 66 85 C0 0F 84 6C 01 00 00 8B 5C 24 08 8B 0E 0F B7 6C 24 0E 66 39 6B 04 0F 86 FF 00 00 00 0F B7 \
     DD 8B 6C 24 08 8D 1C 5B 8B 6D 00 8D 5C 9D 00 39 0B 0F 82 46 01 00 00 0F 87 E0 00 00 00 0F B7 53 08 \
     66 85 D2 0F 84 04 01 00 00 01 C2 83 EC 0C C1 E2 02 52 E8 ?? ?? ?? ?? 83 C4 10 89 C5 85 C0 0F 84 28 \
     01 00 00 0F B7 43 08 8B 53 04 83 EC 04 C1 E0 02 50 52 55 E8 ?? ?? ?? ?? 0F B7 43 08 8B 4E 04 83 C4 \
     0C 8D 54 85 00 0F B7 46 08 C1 E0 02 50 51 52 E8 ?? ?? ?? ?? 58 FF 73 04 E8 ?? ?? ?? ?? 0F B7 53 08 \
     89 6B 04 83 C4 10 66 85 D2 74 3E 80 7B 0A 00 75 38 0F B7 C2 05 FF FF FF 3F 8D 0C 85 00 00 00 00 8B \
     44 85 00 3B 05 ?? ?? ?? ?? 73 1D 6B C0 2C 03 05 ?? ?? ?? ?? 74 12 81 78 28 FF FF 00 00 0F 84 B1 00 \
     00 00 8D 74 26 00 90 66 03 56 08 66 89 53 08 8B 44 24 04 66 83 44 24 0E 01 83 C7 01 0F B7 50 04 66 \
     39 D7 0F 82 DE FE FF FF 83 C4 1C 5B 5E 5F 5D C3 8D B6 00 00 00 00 0F B6 46 0A 83 EC 04 50 51 FF 74 \
     24 14 E8 ?? ?? ?? ?? 83 C4 10 89 C3 85 C0 74 D7 0F B7 46 08 66 85 C0 74 B4 0F B7 53 08 66 85 D2 0F \
     85 FC FE FF FF 83 EC 0C FF 73 04 E8 ?? ?? ?? ?? 5A FF 76 04 E8 ?? ?? ?? ?? 83 C4 10 89 43 04 0F B7 \
     46 08 66 89 43 08 EB 83 8D 76 00 83 C7 01 EB 8C 8D 76 00 66 83 44 24 0E 01 EB 81 8D B4 26 00 00 00 \
     00 90 83 C4 1C 5B 5E 5F 5D E9 ?? ?? ?? ?? 8B 54 0D 04 89 50 28 0F B7 53 08 E9 ?? ?? ?? ??";
/// Its first instructions, moved by the hook: push ebp ; mov ebp,edx ; xor ecx,ecx.
const MERGE_ENTRY: usize = 5;

/// CopyShared: mov eax,[esp+4] ; test eax,eax ; je ; lock add dword [eax-4],1 ; ret. The merge inlines its effect.
const COPY_SHARED: [u8; 14] = [0x8B, 0x44, 0x24, 0x04, 0x85, 0xC0, 0x74, 0x05, 0xF0, 0x83, 0x40, 0xFC, 0x01, 0xC3];
/// FreeShared: ... test eax,eax ; je (null: nothing) ; lock sub dword [eax-4],1 ; je free. The merge skips it on
/// null arrays.
const FREE_SHARED: [u8; 16] =
    [0x83, 0xEC, 0x0C, 0x8B, 0x44, 0x24, 0x10, 0x85, 0xC0, 0x74, 0x07, 0xF0, 0x83, 0x68, 0xFC, 0x01];

/// libbyond's proc table globals, which its merge reads inline: the count, and the base of the 44-byte records.
struct ProcTable {
    count: usize,
    base: usize,
}

const PROC_SIZE: usize = 0x2C;

static PROCS: Installed<ProcTable> = Installed::new();
/// libbyond's merge, for the cases the replacement leaves to it.
static ORIGINAL_MERGE: Addr = Addr::new();

/// The insert and the allocator it uses.
fn find_insert(cx: &Ctx) -> Result<(usize, Alloc)> {
    let core = cx.core();
    let f = core.find_unique(INSERT, "sorted map insert")?;
    let fail = f + INSERT_FAIL;
    core.expect(
        fail,
        &[0x66, 0xC7, 0x40, 0x04, 0x00, 0x00, 0xE8],
        "sorted map insert: out-of-memory path not where expected",
    )?;
    let alloc = Alloc {
        realloc: Foreign::at(alloc::realloc as unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void),
        usable: Foreign::at(alloc::malloc_usable_size as unsafe extern "C" fn(*mut c_void) -> usize),
        out_of_memory: Foreign::at(core.call_target(fail + 6)?),
    };
    Ok((f, alloc))
}

fn install_insert(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (f, alloc) = find_insert(cx)?;
    ALLOC.commit(cx, alloc);
    core.jump(f, 5, insert as unsafe extern "C" fn(_, _, _) -> _)?;
    Ok(core.report(&[f]))
}

/// The proc with this id, or null, as the merge's inlined lookup computes it.
extern "C" fn proc_at(id: u32) -> usize {
    let t = PROCS.get();
    // SAFETY: libbyond's proc table globals (found at install).
    unsafe {
        if id >= live_u32(t.count) {
            return 0;
        }
        (live_u32(t.base) as usize).wrapping_add(id as usize * PROC_SIZE)
    }
}

/// `merge_into` for the stub: true if done, false to run libbyond's merge.
///
/// # Safety
/// `dst` is a live map, `src` one or null (BYOND passes them).
unsafe extern "C" fn merge_c(dst: *mut Map, src: *const Map) -> bool {
    // SAFETY: live maps (caller).
    unsafe { merge_into(dst, src) }
}

/// Replaces the merge (regparm: eax = dst, edx = src): the shared merge (aligned stack), or libbyond's if it declines.
///
/// # Safety
/// Only jumped to from the merge's start.
#[unsafe(naked)]
unsafe extern "C" fn merge_stub() {
    naked_asm!(
        "push ebp",
        "mov ebp, esp",
        "push eax",
        "push edx",
        "and esp, -16",
        "sub esp, 8",
        "push edx",
        "push eax",
        "call {merge}",
        "mov edx, dword ptr [ebp - 8]",
        "movzx ecx, al",
        "mov eax, dword ptr [ebp - 4]",
        "mov esp, ebp",
        "pop ebp",
        "test ecx, ecx",
        "je 2f",
        "ret",
        "2:",
        "jmp dword ptr [{original}]",
        merge = sym merge_c,
        original = sym ORIGINAL_MERGE,
    )
}

fn install_merge(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (insert_fn, alloc) = find_insert(cx)?;
    let m = core.find_unique(MERGE_FN, "sorted map merge")?;
    let call = |at: usize| core.call_target(m + at);
    let jmp = |at: usize| core.branch_at(m + at).map(|b| b.target);
    // libbyond's own helpers; memcpy as the calls bind it.
    let export = |name| core.export(name).unwrap_or(0);
    let (alloc_shared, free_shared, copy_shared, memcpy) =
        (export(c"_Z11AllocSharedm"), export(c"_Z10FreeSharedPv"), export(c"_Z10CopySharedPv"), symbol(c"memcpy"));
    ensure!(
        ![alloc_shared, free_shared, copy_shared, memcpy].contains(&0)
            && call(0x96)? == alloc_shared
            && [call(0xB8)?, call(0xD5)?] == [memcpy, memcpy]
            && [call(0xDE)?, call(0x197)?] == [free_shared, free_shared]
            && call(0x1A0)? == copy_shared,
        "sorted map merge: unexpected value array helpers"
    );
    core.expect(copy_shared, &COPY_SHARED, "sorted map merge: value array helpers not as expected")?;
    core.expect(free_shared, &FREE_SHARED, "sorted map merge: value array helpers not as expected")?;
    ensure!(
        call(0x16D)? == insert_fn && jmp(0x1D7)? == alloc.out_of_memory.addr() && jmp(0x1E7)? == m + 0x130,
        "sorted map merge: unexpected insert or out-of-memory handler"
    );
    PROCS.commit(cx, ProcTable { count: core.ptr_at(m + 0x10D)?, base: core.ptr_at(m + 0x118)? });
    ALLOC.commit(cx, alloc);
    MERGE.commit(
        cx,
        Merge {
            malloc: Foreign::at(alloc::malloc as unsafe extern "C" fn(usize) -> *mut c_void),
            free: Foreign::at(alloc::free as unsafe extern "C" fn(*mut c_void)),
            copy_shared: Foreign::at(copy_shared),
            free_shared: Foreign::at(free_shared),
            alloc_shared: Foreign::at(alloc_shared),
            proc_at: Foreign::at(proc_at as extern "C" fn(u32) -> usize),
        },
    );
    ORIGINAL_MERGE.set(core.hook(m, MERGE_ENTRY, merge_stub as unsafe extern "C" fn())?);
    Ok(core.report(&[m]))
}

/// Both replacements as one group: the merge first (it finds the insert by its unpatched code); either applies alone.
pub(super) fn install_all(cx: &Ctx) -> Result<String> {
    let mut parts = cx.parts();
    parts.part(|| install_merge(cx).map(|d| format!("merge {d}")).map_err(|e| format!("merge: {e}").into()));
    parts.part(|| install_insert(cx).map(|d| format!("insert {d}")).map_err(|e| format!("insert: {e}").into()));
    parts.finish()
}
