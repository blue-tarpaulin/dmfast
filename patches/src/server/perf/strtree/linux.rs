//! `strtree`, Linux: libbyond inlines the tree walk into each of its users (intern, the .dmb loader, find, and
//! removal); each walk is replaced by a call of [`lookup`].

use std::arch::naked_asm;

use super::verify::removed;
use super::{FOUND, Found, ID, LookupFn, lookup, remove};
use crate::base::code::{Addr, Cond, Foreign, hx, live_u32};
use crate::base::driver::verify;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// Intern's walk (esi = &string): lea esi,[esp+20h] ; mov edx,[count] ; mov eax,[table] ; test edx,edx ; je empty ;
/// mov ebx,[eax] ; xor edi,edi ; jmp first ; (pad) ; step: ... ; lea ebp,[ebx+eax*4+8] ; mov ebx,[ebp] ;
/// test ebx,ebx ; jz miss ; first: sub esp,8 ; push ebx ; push esi ; call compare ; add esp,10h ; test eax,eax ;
/// je hit ; (step limit)
const INTERN: &str = "8D 74 24 20 8B 15 ?? ?? ?? ?? A1 ?? ?? ?? ?? 85 D2 74 ?? 8B 18 31 FF EB ?? 8D B4 26 00 00 00 00 66 90 \
                      85 C0 0F 9F C0 0F B6 C0 8D 6C 83 08 8B 5D 00 85 DB 0F 84 ?? ?? ?? ?? 83 EC 08 53 56 E8 ?? ?? ?? ?? \
                      83 C4 10 85 C0 74 ?? 83 C7 01 81 FF E9 03 00 00 75 ?? 83 3D ?? ?? ?? ?? FF 0F 85";
/// Replaced by a jump: `mov edx,[count]`.
const INTERN_AT: usize = 4;
const INTERN_COUNT: usize = 6;
const INTERN_TABLE: usize = 0xB;
const INTERN_EMPTY_JE: usize = 0x11;
const INTERN_MISS_JZ: usize = 0x33;
const INTERN_CALL: usize = 0x3E;
const INTERN_HIT_JE: usize = 0x48;
/// The loader's walk (ebx = entry, its own key): mov edx,[count] ; test edx,edx ; je done ; mov ebp,[eax] ; ... ;
/// (miss) mov [eax],ebx ; done:
const LOADER: &str = "8B 15 ?? ?? ?? ?? 85 D2 74 ?? 8B 28 31 FF EB ?? 8D B4 26 00 00 00 00 85 C0 0F 9F C0 0F B6 C0 \
                      8D 44 85 08 8B 28 85 ED 74 ?? 83 EC 08 55 53 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? 83 C7 01 \
                      81 FF E9 03 00 00 75 ?? 83 3D ?? ?? ?? ?? FF 0F 85 ?? ?? ?? ?? 31 FF EB ?? 8D B4 26 00 00 00 00 66 90 \
                      89 18";
const LOADER_COUNT: usize = 2;
const LOADER_CALL: usize = 0x2E;
/// mov [eax],ebx: links the entry.
const LOADER_LINK: usize = 0x5F;
const LOADER_DONE: usize = 0x61;
/// find (cdecl: string -> id): returns 0xFFFF for null, "", an empty table, or a miss, else the entry's id.
const FIND: &str = "57 56 53 83 EC 20 8B 44 24 30 85 C0 74 ?? 80 38 00 74 ?? 89 04 24 89 E6 8B 15 ?? ?? ?? ?? A1 ?? ?? ?? ?? \
                    85 D2 74 ?? 8B 18 31 FF EB ?? 8D 76 00 85 C0 0F 9F C0 0F B6 C0 8B 5C 83 08 85 DB 74 ?? 83 EC 08 53 56 \
                    E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 ?? 83 C7 01 81 FF E9 03 00 00 75 ?? 83 3D ?? ?? ?? ?? FF 75 ?? 31 FF";
const FIND_COUNT: usize = 0x1A;
const FIND_TABLE: usize = 0x1F;
const FIND_CALL: usize = 0x45;
/// Its `je`s to the miss (null, "", empty table, end of the walk) and to the hit.
const FIND_MISS_JES: [usize; 4] = [0x0C, 0x11, 0x25, 0x3E];
const FIND_HIT_JE: usize = 0x4F;
/// add esp,20h ; mov eax,0FFFFh ; pop ebx ; pop esi ; pop edi ; ret
const FIND_MISS: usize = 0x77;
const FIND_MISS_CODE: &str = "83 C4 20 B8 FF FF 00 00 5B 5E 5F C3";
/// mov eax,[ebx+4] ; add esp,20h ; pop ebx ; pop esi ; pop edi ; ret
const FIND_HIT: usize = 0x88;
const FIND_HIT_CODE: &str = "8B 43 04 83 C4 20 5B 5E 5F C3";
/// Removal's walk (esi = entry): mov edx,[count] ; mov eax,[table] ; test edx,edx ; je empty ; mov ebx,[eax] ; ... ;
/// lea edi,[ebx+eax*4+8] ; mov ebx,[edi] ; test ebx,ebx ; jz not_found ; ... ; jne rebuild ; ... ;
/// found: cmp esi,ebx ; jne not_found ; (unlink)
const REMOVAL: &str = "8B 15 ?? ?? ?? ?? A1 ?? ?? ?? ?? 85 D2 0F 84 ?? ?? ?? ?? 8B 18 31 FF 31 ED EB ?? 8D B4 26 00 00 00 00 \
                       85 C0 0F 9F C0 0F B6 C0 8D 7C 83 08 8B 1F 85 DB 0F 84 ?? ?? ?? ?? 83 EC 08 53 56 E8 ?? ?? ?? ?? \
                       83 C4 10 85 C0 74 ?? 83 C5 01 81 FD E9 03 00 00 75 ?? 83 3D ?? ?? ?? ?? FF 0F 85 ?? ?? ?? ?? \
                       31 ED EB ?? 8D 74 26 00 90 39 DE 0F 85";
const REMOVAL_COUNT: usize = 2;
const REMOVAL_TABLE: usize = 7;
const REMOVAL_EMPTY_JE: usize = 0xD;
const REMOVAL_MISS_JZ: usize = 0x32;
const REMOVAL_CALL: usize = 0x3D;
const REMOVAL_REBUILD_JNE: usize = 0x5B;
/// The found path's `jne not_found` after `cmp esi,ebx`, then the unlink: test edi,edi ; je unlinked.
const REMOVAL_FOUND_JNE: usize = 0x6C;
const REMOVAL_UNLINK: usize = 0x72;
/// How far removal's unlink end and not-found tail lie.
const REMOVAL_LEN: usize = 0x300;
/// Each walk's step limit: add reg,1 ; cmp reg,3E9h ; jne loop ; cmp dword [checksum],-1
const STEP_LIMIT: &str = "81 ?? E9 03 00 00 75 ?? 83 3D ?? ?? ?? ?? FF";
/// What the walks call: strcmp of the strings the two arguments point to.
const COMPARE: &str = "8B 44 24 08 8B 00 89 44 24 08 8B 44 24 04 8B 00 89 44 24 04";
/// The rebuild, after its loop: mov edx,[ebp-40h] ; mov esi,[ebp-48h] ; cmp edx,esi ; je free (the sort and relink
/// are skipped only for an empty list). Fast mode never reads links, so its `je` always jumps.
const REBUILD_END: &str = "8B 55 C0 8B 75 B8 39 F2 74 66";
const REBUILD_SKIP: usize = 0x99;
const SKIP_JE: usize = 8;

/// The count global's address, for [`intern_stub`].
static COUNT: Addr = Addr::new();
/// Intern's hit (ebx = entry), miss (ebp = empty link), and empty table.
static INTERN_HIT: Addr = Addr::new();
static INTERN_MISS: Addr = Addr::new();
static INTERN_EMPTY: Addr = Addr::new();
/// The loader's link (eax = link) and the end of its walk.
static LOADER_LINK_AT: Addr = Addr::new();
static LOADER_DONE_AT: Addr = Addr::new();
/// Removal: past the unlink and the not-found tail (fast mode); the unlink (edi = link, ebx = entry) and the
/// not-found walk (verify mode).
static UNLINKED: Addr = Addr::new();
static NOT_FOUND: Addr = Addr::new();
static UNLINK: Addr = Addr::new();
static NOT_FOUND_WALK: Addr = Addr::new();
static REBUILD: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// What the inlined walks do past 1000 steps (verify mode): rebuild, then walk again.
unsafe extern "C" fn rebuild_and_lookup(key: *const *const u8, out: *mut usize) -> usize {
    // SAFETY: libbyond's rebuild (install); the walk's caller's arguments.
    unsafe {
        REBUILD.get()();
        lookup(key, out)
    }
}

/// Replaces find (cdecl: string -> id, 0xFFFF if absent or empty).
unsafe extern "C" fn find(s: *const u8) -> u32 {
    // SAFETY: `s` is null or a C string (find's caller); a found entry is live.
    unsafe {
        if s.is_null() || *s == 0 {
            return 0xFFFF;
        }
        let mut out = 0usize;
        match lookup(&s, &mut out) {
            0 => 0xFFFF,
            e => live_u32(e + ID),
        }
    }
}

/// Intern's walk (esi = &string); the stack is 16-byte aligned here, as at the walk's own calls.
///
/// # Safety
/// Only jumped to from intern's walk.
#[unsafe(naked)]
unsafe extern "C" fn intern_stub() {
    naked_asm!(
        "sub esp, 8",
        "mov eax, esp",
        "push eax",
        "push esi",
        "call {lookup}",
        "add esp, 8",
        "mov ebp, dword ptr [esp]",
        "add esp, 8",
        "test eax, eax",
        "jz 2f",
        "mov ebx, eax",
        "jmp dword ptr [{hit}]",
        "2:",
        "mov eax, dword ptr [{count}]",
        "cmp dword ptr [eax], 0",
        "je 3f",
        "jmp dword ptr [{miss}]",
        "3:",
        "jmp dword ptr [{empty}]",
        lookup = sym lookup,
        count = sym COUNT,
        hit = sym INTERN_HIT,
        miss = sym INTERN_MISS,
        empty = sym INTERN_EMPTY,
    )
}

/// The loader's walk (ebx = entry, looked up by its own string): links it on a miss.
///
/// # Safety
/// Only jumped to from the loader's walk.
#[unsafe(naked)]
unsafe extern "C" fn loader_stub() {
    naked_asm!(
        "sub esp, 8",
        "mov eax, esp",
        "push eax",
        "push ebx",
        "call {lookup}",
        "add esp, 8",
        "mov ecx, dword ptr [esp]",
        "add esp, 8",
        "test eax, eax",
        "jnz 2f",
        "test ecx, ecx",
        "jz 2f",
        "mov eax, ecx",
        "jmp dword ptr [{link}]",
        "2:",
        "jmp dword ptr [{done}]",
        lookup = sym lookup,
        link = sym LOADER_LINK_AT,
        done = sym LOADER_DONE_AT,
    )
}

/// Removal's walk (esi = entry), fast mode: past the unlink (ebx = entry) if [`remove`] dropped it, else the
/// not-found tail.
///
/// # Safety
/// Only jumped to from removal's walk.
#[unsafe(naked)]
unsafe extern "C" fn remove_stub() {
    naked_asm!(
        "sub esp, 12",
        "push esi",
        "call {remove}",
        "add esp, 16",
        "test eax, eax",
        "jz 2f",
        "mov ebx, esi",
        "jmp dword ptr [{unlinked}]",
        "2:",
        "jmp dword ptr [{not_found}]",
        remove = sym remove,
        unlinked = sym UNLINKED,
        not_found = sym NOT_FOUND,
    )
}

/// Removal's walk, verify mode: the checked lookup, then libbyond's own unlink or not-found walk.
///
/// # Safety
/// Only jumped to from removal's walk.
#[unsafe(naked)]
unsafe extern "C" fn removed_stub() {
    naked_asm!(
        "sub esp, 8",
        "mov eax, esp",
        "push eax",
        "push esi",
        "call {lookup}",
        "add esp, 8",
        "mov edi, dword ptr [esp]",
        "mov ebx, eax",
        "push eax",
        "push esi",
        "call {removed}",
        "add esp, 16",
        "cmp ebx, esi",
        "jne 2f",
        "jmp dword ptr [{unlink}]",
        "2:",
        "jmp dword ptr [{walk}]",
        lookup = sym lookup,
        removed = sym removed,
        unlink = sym UNLINK,
        walk = sym NOT_FOUND_WALK,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let i = core.find_unique(INTERN, "string intern walk")?;
    let ld = core.find_unique(LOADER, "string loader walk")?;
    let fd = core.find_unique(FIND, "string find")?;
    let rm = core.find_unique(REMOVAL, "string removal walk")?;
    let (count, table, checksum) = check_walks(core, i, ld, fd, rm)?;
    check_find(core, fd)?;
    let (rebuild, loading) = removal(core, rm, table)?;

    COUNT.set(count);
    REBUILD.set(rebuild);
    INTERN_HIT.set(core.jcc_at(i + INTERN_HIT_JE, Cond::Equal)?.target);
    INTERN_MISS.set(core.jcc_at(i + INTERN_MISS_JZ, Cond::Zero)?.target);
    INTERN_EMPTY.set(core.jcc_at(i + INTERN_EMPTY_JE, Cond::Equal)?.target);
    LOADER_LINK_AT.set(ld + LOADER_LINK);
    LOADER_DONE_AT.set(ld + LOADER_DONE);
    let original = Foreign::at(rebuild_and_lookup as LookupFn);
    FOUND.commit(cx, Found { table, count, checksum, loading, original });

    core.jump(i + INTERN_AT, 5, intern_stub as *const ())?;
    core.jump(ld, 6, loader_stub as *const ())?;
    core.jump(fd, 5, find as unsafe extern "C" fn(*const u8) -> u32)?;
    if verify() {
        core.jump(rm, 6, removed_stub as *const ())?;
    } else {
        core.jump(rm, 6, remove_stub as *const ())?;
        core.always_jump(rebuild + REBUILD_SKIP + SKIP_JE)?;
    }
    Ok(core.report(&[i, ld, fd, rm]))
}

/// Removal's walk at `rm`: where its stubs go on (the unlink, its end, the not-found walk, and the not-found tail; the
/// end and the tail both `mov eax,[table] ; mov edx,[esp+0Ch] ; mov dword [eax+edx*4],0`). The rebuild it calls,
/// and the .dmb loader's flag the rebuild reads.
fn removal(core: &Image, rm: usize, table: usize) -> Result<(usize, usize)> {
    let rebuild = core.call_target(core.jcc_at(rm + REMOVAL_REBUILD_JNE, Cond::NotEqual)?.target)?;
    let tails = core.find_in(rm, REMOVAL_LEN, &format!("A1 {} 8B 54 24 0C C7 04 90 00 00 00 00", hx(table)))?;
    let [unlinked, not_found] = tails.as_slice()[..] else {
        bail!("string removal: unlink end and not-found tail not found");
    };
    let not_found_walk = core.jcc_at(rm + REMOVAL_MISS_JZ, Cond::Zero)?.target;
    ensure!(
        core.jcc_at(rm + REMOVAL_FOUND_JNE, Cond::NotEqual)?.target == not_found_walk
            && core.jcc_at(rm + REMOVAL_EMPTY_JE, Cond::Equal)?.target == not_found + 5,
        "string removal: unexpected not-found paths"
    );
    let unlink = rm + REMOVAL_UNLINK;
    core.require(unlink, "85 FF", "string removal unlink")?;
    ensure!(core.jcc_at(unlink + 2, Cond::Equal)?.target == unlinked, "string removal: unlink not where expected");
    core.require(rebuild + REBUILD_SKIP, REBUILD_END, "string tree rebuild")?;
    let flag = core.find_unique_in(rebuild, 0x80, "80 3D ?? ?? ?? ?? 00", "string tree rebuild loading flag")?;
    UNLINKED.set(unlinked);
    NOT_FOUND.set(not_found);
    UNLINK.set(unlink);
    NOT_FOUND_WALK.set(not_found_walk);
    Ok((rebuild, core.ptr_at(flag + 2)?))
}

/// The four walks read the same globals and call the same compare: (count, table, checksum).
fn check_walks(core: &Image, i: usize, ld: usize, fd: usize, rm: usize) -> Result<(usize, usize, usize)> {
    let (count, table) = (core.ptr_at(i + INTERN_COUNT)?, core.ptr_at(i + INTERN_TABLE)?);
    for (at, global) in [
        (ld + LOADER_COUNT, count),
        (fd + FIND_COUNT, count),
        (fd + FIND_TABLE, table),
        (rm + REMOVAL_COUNT, count),
        (rm + REMOVAL_TABLE, table),
    ] {
        ensure!(core.ptr_at(at)? == global, "string tree walks: globals differ");
    }
    let compare = core.call_target(i + INTERN_CALL)?;
    for at in [ld + LOADER_CALL, fd + FIND_CALL, rm + REMOVAL_CALL] {
        ensure!(core.call_target(at)? == compare, "string tree walks: unexpected compare");
    }
    core.require(compare, COMPARE, "string tree compare")?;
    let step_limit = |walk: usize| {
        let at = core.find_unique_in(walk, 0x70, STEP_LIMIT, "string tree walk step limit")?;
        core.ptr_at(at + 10)
    };
    let checksum = step_limit(i)?;
    for w in [ld, fd, rm] {
        ensure!(step_limit(w)? == checksum, "string tree walks: checksum globals differ");
    }
    Ok((count, table, checksum))
}

/// find's results, which [`find`] gives: 0xFFFF on a miss, the entry's id on a hit.
fn check_find(core: &Image, fd: usize) -> Result<()> {
    core.require(fd + FIND_MISS, FIND_MISS_CODE, "string find miss")?;
    core.require(fd + FIND_HIT, FIND_HIT_CODE, "string find hit")?;
    let err = "string find: unexpected results";
    for at in FIND_MISS_JES {
        ensure!(core.jcc_at(fd + at, Cond::Equal)?.target == fd + FIND_MISS, "{err}");
    }
    ensure!(core.jcc_at(fd + FIND_HIT_JE, Cond::Equal)?.target == fd + FIND_HIT, "{err}");
    Ok(())
}
