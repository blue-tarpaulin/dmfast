//! `slots`, Linux: the shared allocator body (two tables), its inlined pproc copy, a third table's allocator, and the
//! hot inlined mpop scan (its other copies only fill slots) each jump from their scan's setup to a stub that asks
//! [`first_free`] where to start and enters the original loop there, as from slot 0. The third table's setter can
//! empty a slot, so it lowers that table's hint.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::{Addr, Foreign, live_u32};
use crate::base::driver::{Counter, verify};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

/// Table ids (hint indexes): the shared body's two tables (address order), the third table, and the mpop table.
const TABLES: usize = 4;
const THIRD: usize = 2;
const MPOP: usize = 3;

/// The shared body: mov esi,[count] ; mov ecx,[table] ; test esi,esi ; je new ; mov eax,ecx ; xor ebx,ebx ; jmp loop
const BODY: &str = "8B 35 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 85 F6 0F 84 ?? ?? ?? ?? 89 C8 31 DB EB";
/// Its loop: mov ebp,[eax] ; lea edi,[ebx*4] ; mov edx,eax ; test ebp,ebp ; jne
const BODY_LOOP_CODE: &str = "8B 28 8D 3C 9D 00 00 00 00 89 C2 85 ED 75";
/// `body_stub!` replaces `mov eax,ecx ; xor ebx,ebx ; jmp loop` (its rel8 field at +5).
const BODY_SITE: usize = 20;
const BODY_SITE_LEN: usize = 6;
const BODY_JMP_LOOP: usize = 5;
/// The pproc copy: mov ecx,[table] ; mov edi,[count] ; xor ebx,ebx ; mov eax,ecx ; test edi,edi ; jne loop ;
/// mov dword [count],0Ah (empty table)
const PPROC: &str = "8B 0D ?? ?? ?? ?? 8B 3D ?? ?? ?? ?? 31 DB 89 C8 85 FF 75 ?? C7 05 ?? ?? ?? ?? 0A 00 00 00";
/// Its loop: cmp dword [eax],0 ; lea ebp,[ebx*4] ; mov edx,eax ; jne next
const PPROC_LOOP_CODE: &str = "83 38 00 8D 2C 9D 00 00 00 00 89 C2 75";
/// `inline_stub!` replaces `xor ebx,ebx ; mov eax,ecx ; test edi,edi ; jne loop` (its rel8 field at +7).
const PPROC_SITE: usize = 12;
const PPROC_SITE_LEN: usize = 8;
const PPROC_JNE_LOOP: usize = 7;
/// The third table's allocator: push ebx ; sub esp,8 ; mov eax,[count] ; test eax,eax ; je ; mov edx,[table] ;
/// xor ebx,ebx ; jmp loop ; (pad) ; next: add ebx,1 ; cmp ebx,eax ; je after ; loop: mov ecx,[edx+ebx*4] ;
/// test ecx,ecx ; jne next ; after: cmp ebx,0FFFFh
const THIRD_ALLOC: &str = "53 83 EC 08 A1 ?? ?? ?? ?? 85 C0 74 ?? 8B 15 ?? ?? ?? ?? 31 DB EB ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? \
                           83 C3 01 39 C3 74 ?? 8B 0C 9A 85 C9 75 ?? 81 FB FF FF 00 00";
/// [`third_stub`] replaces `mov edx,[table] ; xor ebx,ebx ; jmp loop`; it continues at `after`.
const THIRD_SITE: usize = 13;
const THIRD_SITE_LEN: usize = 10;
const THIRD_AFTER_AT: usize = 0x2E;
/// Its setter's entry: push ebp ; push edi ; push esi ; push ebx ; sub esp,1Ch
const SETTER_ENTRY: &str = "55 57 56 53 83 EC 1C";
/// The mpop scan: mov edx,[count] ; test edx,edx ; je ; mov ecx,[table] ; xor eax,eax ; jmp loop ; (pad) ;
/// next: add eax,1 ; cmp eax,edx ; je after ; loop: mov ebx,[ecx+eax*4] ; test ebx,ebx ; jne next ;
/// after: cmp eax,0FFFFh ; je ; sub esp,8 ; push eax ; mov [esp+18h],eax ; push esi ; call setter
const MPOP_SCAN: &str = "8B 15 ?? ?? ?? ?? 85 D2 74 ?? 8B 0D ?? ?? ?? ?? 31 C0 EB 0B 8D 74 26 00 83 C0 01 39 D0 74 07 \
                         8B 1C 81 85 DB 75 F2 3D FF FF 00 00 74 ?? 83 EC 08 50 89 44 24 18 56 E8";
/// [`mpop_stub`] replaces `mov ecx,[table] ; xor eax,eax ; jmp loop`; it continues at `after`.
const MPOP_SITE: usize = 10;
const MPOP_SITE_LEN: usize = 10;
const MPOP_AFTER_AT: usize = 0x26;
const MPOP_SETTER_CALL: usize = 0x36;

static HINTS: [AtomicU32; TABLES] = [const { AtomicU32::new(0) }; TABLES];
/// Where the stubs continue, and the table address globals they read.
static BODY_LOOP: [Addr; 2] = [const { Addr::new() }; 2];
static PPROC_LOOP: Addr = Addr::new();
static PPROC_EMPTY: Addr = Addr::new();
static THIRD_TABLE: Addr = Addr::new();
static THIRD_AFTER: Addr = Addr::new();
static MPOP_TABLE: Addr = Addr::new();
static MPOP_AFTER: Addr = Addr::new();

/// The third table's setter (cdecl: object, slot).
type SetterFn = unsafe extern "C" fn(usize, u32);

/// What [`setter`] needs: the original setter and the globals holding the table's count and address.
struct Third {
    original: Foreign<SetterFn>,
    count: usize,
    table: usize,
}

static THIRD_SETTER: Installed<Third> = Installed::new();

static CALLS: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();
static LOWERED: Counter = Counter::new();

/// The first null slot of `table[0..count)` from table `id`'s hint, or `count`; it becomes the hint.
///
/// # Safety
/// `table` holds `count` slots (the stubs pass the table's own globals).
unsafe extern "C" fn first_free(id: usize, table: usize, count: u32) -> u32 {
    // SAFETY: below `count` (caller).
    let used = |i: u32| unsafe { live_u32(table + 4 * i as usize) } != 0;
    let mut h = HINTS[id].load(Relaxed);
    if h > count || (h > 0 && !used(h - 1)) {
        h = 0;
    }
    let r = (h..count).find(|&i| !used(i)).unwrap_or(count);
    CALLS.bump();
    if verify() && (0..count).find(|&i| !used(i)).unwrap_or(count) != r {
        MISMATCHES.bump();
    }
    HINTS[id].store(r, Relaxed);
    r
}

/// Shared allocator body, entered instead of `mov eax,ecx ; xor ebx,ebx ; jmp loop` with esi = count
/// (nonzero), ecx = table. The loop needs a slot inside the table (it handles a full table itself from
/// the last slot), with ebx = slot, eax = `&table[ebx]`; eax, ebx, edx, edi, ebp are free.
macro_rules! body_stub {
    ($name:ident, $id:literal) => {
        /// # Safety
        /// Only jumped to from a shared allocator body.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "pushad",
                "mov ebp, esp",
                "and esp, -16",
                "sub esp, 4",
                "push esi",
                "push ecx",
                "push {id}",
                "call {first_free}",
                "mov esp, ebp",
                "mov dword ptr [esp + 16], eax", // popad restores it into ebx
                "popad",
                "lea eax, [esi - 1]",
                "cmp ebx, eax",
                "jbe 2f",
                "mov ebx, eax",
                "2:",
                "lea eax, [ecx + ebx*4]",
                "jmp dword ptr [{back} + 4*{id}]",
                id = const $id,
                first_free = sym first_free,
                back = sym BODY_LOOP,
            )
        }
    };
}

body_stub!(body0, 0);
body_stub!(body1, 1);

/// Inlined pproc scan of table `$id`, entered instead of `xor ebx,ebx ; mov eax,ecx ; test edi,edi ; jne loop` with
/// edi = count, ecx = table. Loop contract as the body's; an empty table takes the count == 0 path with ebx = 0,
/// eax = table.
macro_rules! inline_stub {
    ($name:ident, $id:literal) => {
        /// # Safety
        /// Only jumped to from the inlined pproc scan.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "test edi, edi",
                "jnz 2f",
                "xor ebx, ebx",
                "mov eax, ecx",
                "jmp dword ptr [{empty}]",
                "2:",
                "pushad",
                "mov ebp, esp",
                "and esp, -16",
                "sub esp, 4",
                "push edi",
                "push ecx",
                "push {id}",
                "call {first_free}",
                "mov esp, ebp",
                "mov dword ptr [esp + 16], eax",
                "popad",
                "lea eax, [edi - 1]",
                "cmp ebx, eax",
                "jbe 3f",
                "mov ebx, eax",
                "3:",
                "lea eax, [ecx + ebx*4]",
                "jmp dword ptr [{back}]",
                id = const $id,
                empty = sym PPROC_EMPTY,
                first_free = sym first_free,
                back = sym PPROC_LOOP,
            )
        }
    };
}

inline_stub!(inline0, 0);
inline_stub!(inline1, 1);

/// Third table's allocator, entered instead of `mov edx,[table] ; xor ebx,ebx ; jmp loop` with
/// eax = count (nonzero). Continues after its loop with ebx = the first free slot or count (it grows the
/// table through the setter), edx = table, eax = count.
///
/// # Safety
/// Only jumped to from the third table's allocator.
#[unsafe(naked)]
unsafe extern "C" fn third_stub() {
    naked_asm!(
        "pushad",
        "mov ebp, esp",
        "and esp, -16",
        "sub esp, 4",
        "push eax",
        "mov ecx, dword ptr [{table}]",
        "push dword ptr [ecx]",
        "push {third}",
        "call {first_free}",
        "mov esp, ebp",
        "mov dword ptr [esp + 16], eax",
        "popad",
        "mov edx, dword ptr [{table}]",
        "mov edx, dword ptr [edx]",
        "jmp dword ptr [{after}]",
        table = sym THIRD_TABLE,
        third = const THIRD,
        first_free = sym first_free,
        after = sym THIRD_AFTER,
    )
}

/// Replaces the third table's setter: run it, then lower the hint if that slot is now empty.
///
/// # Safety
/// As the original setter: `obj` is the compiler's object.
unsafe extern "C" fn setter(obj: usize, slot: u32) {
    let t = THIRD_SETTER.get();
    // SAFETY: the caller's arguments; the table's globals (install), and `slot` is checked against the count.
    unsafe {
        t.original.get()(obj, slot);
        let table = live_u32(t.table) as usize;
        if slot < live_u32(t.count) && slot < HINTS[THIRD].load(Relaxed) && live_u32(table + 4 * slot as usize) == 0 {
            HINTS[THIRD].store(slot, Relaxed);
            LOWERED.bump();
        }
    }
}

/// Inlined mpop scan, entered instead of `mov ecx,[table] ; xor eax,eax ; jmp loop` with edx = count (nonzero).
/// Leaves the registers as the loop would: eax = slot (or count), ecx = table, edx = count, ebx = the last entry
/// loaded (0 at a free slot, table[count-1] if full); continues at the loop's exit (`cmp eax,0FFFFh`).
///
/// # Safety
/// Only jumped to from the inlined mpop scan.
#[unsafe(naked)]
unsafe extern "C" fn mpop_stub() {
    naked_asm!(
        "pushad",
        "mov ebp, esp",
        "and esp, -16",
        "sub esp, 4",
        "push edx",
        "mov ecx, dword ptr [{table}]",
        "push dword ptr [ecx]",
        "push {mpop}",
        "call {first_free}",
        "mov esp, ebp",
        "mov dword ptr [esp + 28], eax", // popad restores it into eax
        "popad",
        "mov ecx, dword ptr [{table}]",
        "mov ecx, dword ptr [ecx]",
        "xor ebx, ebx",
        "cmp eax, edx",
        "jb 2f",
        "mov ebx, dword ptr [ecx + edx*4 - 4]",
        "2:",
        "jmp dword ptr [{after}]",
        table = sym MPOP_TABLE,
        mpop = const MPOP,
        first_free = sym first_free,
        after = sym MPOP_AFTER,
    )
}

/// The shared bodies; returns their tables' (count, table) globals, by id.
fn install_bodies(core: &Image) -> Result<[(usize, usize); 2]> {
    let bodies = core.find_all(BODY)?;
    ensure!(bodies.len() == 2, "shared allocator bodies: expected 2, found {}", bodies.len());
    let stubs = [body0 as unsafe extern "C" fn(), body1];
    let mut tables = [(0, 0); 2];
    for (id, &m) in bodies.iter().enumerate() {
        let site = m + BODY_SITE;
        let lp = core.rel8(site + BODY_JMP_LOOP)?;
        core.require(lp, BODY_LOOP_CODE, "shared allocator scan loop")?;
        tables[id] = (core.ptr_at(m + 2)?, core.ptr_at(m + 8)?);
        BODY_LOOP[id].set(lp);
        core.jump(site, BODY_SITE_LEN, stubs[id])?;
    }
    Ok(tables)
}

/// The pproc copy: shares the hint of the body for the same table.
fn install_pproc(core: &Image, bodies: Option<[(usize, usize); 2]>) -> Result<()> {
    let bodies = bodies.ok_or("needs the shared allocators")?;
    let m = core.find_unique(PPROC, "inlined pproc slot scan")?;
    let (table_var, count_var) = (core.ptr_at(m + 2)?, core.ptr_at(m + 8)?);
    let id = bodies
        .iter()
        .position(|&t| t == (count_var, table_var))
        .ok_or("inlined pproc slot scan: table differs from the allocators'")?;
    let site = m + PPROC_SITE;
    let lp = core.rel8(site + PPROC_JNE_LOOP)?;
    core.require(lp, PPROC_LOOP_CODE, "inlined pproc scan loop")?;
    PPROC_LOOP.set(lp);
    PPROC_EMPTY.set(site + PPROC_SITE_LEN);
    let stub = [inline0 as unsafe extern "C" fn(), inline1][id];
    core.jump(site, PPROC_SITE_LEN, stub)
}

/// The third table: its allocator's scan, and its setter, which can empty slots.
fn install_third(cx: &Ctx) -> Result<()> {
    let core = cx.core();
    let f = core.find_unique(THIRD_ALLOC, "third slot allocator")?;
    let (count, table) = (core.ptr_at(f + 5)?, core.ptr_at(f + 15)?);
    // After the loop: ... ; push ebx ; push <placeholder> ; call setter
    let push = core.find_bytes_in(f, 0x60, &[0x53, 0x68])?.first_or("third slot allocator: setter call")?;
    let set = core.call_target(push + 6)?;
    core.require(set, SETTER_ENTRY, "third slot allocator: setter")?;
    ensure!(core.mentions(set, 0x20, count)?, "third slot allocator: setter: another table");
    THIRD_TABLE.set(table);
    THIRD_AFTER.set(f + THIRD_AFTER_AT);
    core.jump(f + THIRD_SITE, THIRD_SITE_LEN, third_stub as unsafe extern "C" fn())?;
    let original = core.hook(set, SETTER_ENTRY.split_whitespace().count(), setter as SetterFn)?;
    THIRD_SETTER.commit(cx, Third { original: Foreign::at(original), count, table });
    Ok(())
}

/// The mpop table: the hot one of its inlined scans. Its setter must use the same table.
fn install_mpop(core: &Image) -> Result<()> {
    let m = core.find_unique(MPOP_SCAN, "mpop slot scan")?;
    let (count, table) = (core.ptr_at(m + 2)?, core.ptr_at(m + 12)?);
    let set = core.call_target(m + MPOP_SETTER_CALL)?;
    ensure!(core.mentions(set, 0x20, count)? && core.mentions(set, 0x20, table)?, "mpop slot scan: unexpected setter");
    MPOP_TABLE.set(table);
    MPOP_AFTER.set(m + MPOP_AFTER_AT);
    core.jump(m + MPOP_SITE, MPOP_SITE_LEN, mpop_stub as unsafe extern "C" fn())
}

/// Each part installs on its own; a part that doesn't match is left out (every hint checks itself).
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut parts = cx.parts();
    let mut bodies = None;
    parts.named("shared allocators", || {
        bodies = Some(install_bodies(core)?);
        Ok(())
    });
    parts.named("inlined pproc scan", || install_pproc(core, bodies));
    parts.named("third table", || install_third(cx));
    parts.named("mpop scan", || install_mpop(core));
    parts.finish()
}

pub(super) fn print_stats() {
    eprintln!("dmfast: slot hints calls={CALLS} lowered={LOWERED} verify={} mismatches={MISMATCHES}", verify());
}
