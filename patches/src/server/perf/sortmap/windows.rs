//! `sortmap`, Windows: byondcore's sorted map insert and merge replaced by the shared ones.

use super::{ALLOC, Alloc, MERGE, Map, Merge, insert, merge_into};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::windows::ucrt::ucrt_export;
use crate::base::{Result, ensure};

/// The insert: push ebp ; mov ebp,esp ; push ecx ; push ebx ; mov ebx,ecx ; ... binary search on u16 lo/hi over
/// 12-byte entries ; (miss) movzx eax,[ebx+4] ; inc eax ; lea eax,[eax+eax*2] ; shl eax,2 ; push eax ; push [ebx] ;
/// call [realloc] ; ... ; jne ok ; mov [ebx+4],ax ; call out_of_memory
const INSERT: &str = "55 8B EC 51 53 8B D9 33 C9 56 33 F6 57 0F B7 43 04 8B D0 66 3B CA 73 36 8B 3B 0F B7 C6 0F B7 CA 03 C8 \
                      D1 E9 0F B7 C9 8B C1 8D 04 40 8D 04 87 89 45 FC 8B 00 3B 45 08 73 08 8D 41 01 0F B7 F0 EB 04 76 3D \
                      8B D1 66 3B F2 72 CE 0F B7 43 04 0F B7 C0 40 8D 04 40 C1 E0 02 50 FF 33 FF 15 ?? ?? ?? ?? 8B D0 83 C4 08 \
                      89 13 85 D2 75 20 66 89 43 04 E8";
/// Where the insert calls the out-of-memory handler.
const INSERT_OOM: usize = 0x70;

/// The whole merge: per source entry with values, skip smaller destination keys, insert the key if absent (call
/// insert ; esi = entry), then share the source array (FreeShared ; CopyShared) or join both (alloc ; memcpy ;
/// memcpy ; FreeShared) and link the last old value's proc (proc_at ; cmp [eax+28h],0FFFFh) to the new.
const MERGE_FN: &str = "55 8B EC 83 EC 10 56 57 8B 7D 08 8B F1 89 75 F8 85 FF 0F 84 7F 01 00 00 0F B7 47 04 66 85 C0 0F 84 72 \
     01 00 00 53 33 D2 33 C9 33 DB 89 55 FC 66 3B C8 0F 83 5E 01 00 00 0F B7 C3 8D 0C 40 8B 07 66 83 7C 88 \
     08 00 8D 3C 88 0F 84 2B 01 00 00 66 3B 56 04 73 22 0F B7 C2 8D 0C 40 8B 06 8D 34 88 8B 06 3B 07 73 0C \
     8B 75 F8 42 89 55 FC E9 09 01 00 00 76 1E 8B 75 F8 0F B6 47 0A 8B CE 50 FF 37 E8 ?? ?? ?? ?? 8B F0 85 \
     F6 0F 84 07 01 00 00 8B 55 FC 0F B7 47 08 66 85 C0 0F 84 D4 00 00 00 0F B7 4E 08 66 85 C9 75 1F FF 76 \
     04 E8 ?? ?? ?? ?? FF 77 04 E8 ?? ?? ?? ?? 89 46 04 83 C4 08 66 8B 47 08 E9 A5 00 00 00 03 C8 C1 E1 02 \
     51 E8 ?? ?? ?? ?? 83 C4 04 89 45 F4 85 C0 0F 84 B1 00 00 00 0F B7 4E 08 C1 E1 02 51 FF 76 04 50 E8 ?? \
     ?? ?? ?? 0F B7 47 08 8B 4D F4 C1 E0 02 50 0F B7 46 08 FF 77 04 8D 04 81 50 E8 ?? ?? ?? ?? FF 76 04 E8 \
     ?? ?? ?? ?? 0F B7 46 08 83 C4 1C 8B 55 F4 8B C8 89 56 04 66 85 C0 74 3D 80 7E 0A 00 75 37 FF 74 82 FC \
     8D 04 82 89 45 F4 E8 ?? ?? ?? ?? 0F B7 56 08 83 C4 04 89 45 F0 8B CA 85 C0 74 18 81 78 28 FF FF 00 00 \
     75 0F 8B 45 F4 8B 4D F0 8B 00 89 41 28 0F B7 4E 08 66 8B 47 08 66 03 C1 8B 55 FC 66 89 46 08 8B 75 F8 \
     42 89 55 FC 43 8B 7D 08 66 3B 5F 04 0F 82 B0 FE FF FF 5B 5F 5E 8B E5 5D C2 04 00 E8 ?? ?? ?? ?? 5B 5F \
     5E 8B E5 5D";
/// Its first instructions, moved by the hook: push ebp ; mov ebp,esp ; sub esp,10h.
const MERGE_ENTRY: usize = 6;

/// The helpers the merge calls, by their code: CopyShared (lock inc [eax-4]), FreeShared (lock xadd [ecx-4],-1 ;
/// free at zero), the shared-array allocator ({size, 1} header), and the proc table accessor.
const COPY_SHARED: &str = "55 8B EC 8B 45 08 85 C0 74 04 F0 FF 40 FC 5D C3";
const FREE_SHARED: &str = "55 8B EC 8B 4D 08 85 C9 74 1B 83 C8 FF F0 0F C1 41 FC 75 11 8D 41 F8 50 FF 15";
const ALLOC_SHARED: &str =
    "55 8B EC 56 8B 75 08 8D 46 08 50 FF 15 ?? ?? ?? ?? 83 C4 04 85 C0 74 0C 89 30 C7 40 04 01 00 00 00 83 C0 08";
const PROC_AT: &str = "55 8B EC 8B 45 08 3B 05 ?? ?? ?? ?? 72 04 33 C0 5D C3 6B C0 2C";

/// BYOND's merge, for the cases the replacement leaves to it.
static ORIGINAL_MERGE: Foreign<unsafe extern "thiscall" fn(*mut Map, *const Map)> = Foreign::new();

/// The insert and the allocator it uses.
fn find_insert(cx: &Ctx) -> Result<(usize, Alloc)> {
    let core = cx.core();
    let f = core.find_unique(INSERT, "sorted map insert")?;
    let slot = core.ptr_at(f + 0x5D)?;
    ensure!(slot == core.ucrt_slot("realloc")?, "sorted map insert: does not grow with ucrtbase's realloc");
    let alloc = Alloc {
        realloc: Foreign::at(core.live_ptr(slot)?),
        // ucrtbase's block size query, which dmfast's allocator answers for mimalloc and heap blocks alike.
        usable: Foreign::at(ucrt_export("_msize")?),
        out_of_memory: Foreign::at(core.call_target(f + INSERT_OOM)?),
    };
    Ok((f, alloc))
}

fn install_insert(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (f, alloc) = find_insert(cx)?;
    ALLOC.commit(cx, alloc);
    core.jump(f, 5, insert as unsafe extern "thiscall" fn(_, _, _) -> _)?;
    Ok(core.report(&[f]))
}

/// Replaces the merge (thiscall: this = destination map; source map).
///
/// # Safety
/// `dst` is a live map, `src` one or null (BYOND passes them).
unsafe extern "thiscall" fn merge(dst: *mut Map, src: *const Map) {
    // SAFETY: live maps (caller); ORIGINAL_MERGE is BYOND's merge (set at install).
    unsafe {
        if !merge_into(dst, src) {
            ORIGINAL_MERGE.get()(dst, src);
        }
    }
}

fn install_merge(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (insert_fn, alloc) = find_insert(cx)?;
    let m = core.find_unique(MERGE_FN, "sorted map merge")?;
    let call = |at: usize| core.call_target(m + at);
    let (free_shared, copy_shared, alloc_shared, proc_at) = (call(0xAB)?, call(0xB3)?, call(0xCD)?, call(0x138)?);
    for (at, ida, what) in [
        (copy_shared, COPY_SHARED, "sorted map merge's CopyShared"),
        (free_shared, FREE_SHARED, "sorted map merge's FreeShared"),
        (alloc_shared, ALLOC_SHARED, "sorted map merge's shared-array allocator"),
        (proc_at, PROC_AT, "sorted map merge's proc table accessor"),
    ] {
        core.require(at, ida, what)?;
    }
    ensure!(
        call(0x80)? == insert_fn && call(0x191)? == core.call_target(insert_fn + INSERT_OOM)?,
        "sorted map merge: does not use the sorted map insert"
    );
    ALLOC.commit(cx, alloc);
    // The CRT malloc and free byondcore's helpers call (call [slot] in the allocator and FreeShared).
    MERGE.commit(
        cx,
        Merge {
            malloc: Foreign::at(core.live_ptr(core.ptr_at(alloc_shared + 13)?)?),
            free: Foreign::at(core.live_ptr(core.ptr_at(free_shared + 26)?)?),
            copy_shared: Foreign::at(copy_shared),
            free_shared: Foreign::at(free_shared),
            alloc_shared: Foreign::at(alloc_shared),
            proc_at: Foreign::at(proc_at),
        },
    );
    ORIGINAL_MERGE.set(core.hook(m, MERGE_ENTRY, merge as unsafe extern "thiscall" fn(_, _))?);
    Ok(core.report(&[m]))
}

/// Both replacements as one group: the merge first (it finds the insert by its unpatched code); either applies alone.
pub(super) fn install_all(cx: &Ctx) -> Result<String> {
    let mut parts = cx.parts();
    parts.part(|| install_merge(cx).map(|d| format!("merge {d}")).map_err(|e| format!("merge: {e}").into()));
    parts.part(|| install_insert(cx).map(|d| format!("insert {d}")).map_err(|e| format!("insert: {e}").into()));
    parts.finish()
}
