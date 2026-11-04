//! `slots`, Windows: three allocators share one body, a fourth has the same scan, and pproc inlines a copy; each scan's
//! start jumps to a stub that takes the slot from [`alloc_index`]. The global reset clears the hints first.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::base::code::{Stub, live_u32, trampoline};
use crate::base::driver::{Counter, verify};
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

const MAX_TABLES: usize = 4;
const RESERVED: u32 = 0xFFFF;

/// An allocator: mov eax,[count] ; push esi ; xor esi,esi ; ... scan ... ; push esi ; push <placeholder> ; call setter.
const ALLOC: &str = "A1 ?? ?? ?? ?? 56 33 F6 85 C0 74 2F 8B 0D ?? ?? ?? ?? 83 3C B1 00 74 05 46 3B F0 72 F5 \
                     81 FE FF FF 00 00 75 16 BE 00 00 01 00 3B C6 76 0D 66 90 83 3C B1 00 74 05 46 3B F0 72 F5 \
                     56 68 ?? ?? ?? ?? E8";
/// The fourth: the same scan without the 2-byte nop before its second loop, ending
/// `push esi ; push [ebp+8] ; call setter`. Optional.
const ARGS: &str = "A1 ?? ?? ?? ?? 56 33 F6 85 C0 74 2D 8B 0D ?? ?? ?? ?? 83 3C B1 00 74 05 46 3B F0 72 F5 \
                    81 FE FF FF 00 00 75 14 BE 00 00 01 00 3B C6 76 0B 83 3C B1 00 74 05 46 3B F0 72 F5 \
                    56 FF 75 08 E8";
/// Where each scan ends: `push esi` with the slot.
const ALLOC_USE: usize = 0x3B;
const ARGS_USE: usize = 0x39;
/// In both: the globals holding the count (`mov eax,[count]`, the 5 bytes replaced) and the table's address.
const COUNT_VAR: usize = 1;
const TABLE_VAR: usize = 14;
/// The pproc copy, without `push esi`: mov eax,[count] ; xor esi,esi ; test eax,eax ; je use ; mov ecx,[table] ;
/// ... scan ...
const INLINE: &str = "A1 ?? ?? ?? ?? 33 F6 85 C0 74 ?? 8B 0D ?? ?? ?? ?? 83 3C B1 00 74 05 46 3B F0 72 F5 \
                      81 FE FF FF 00 00 75 ?? BE 00 00 01 00 3B C6 76";
const INLINE_JE_USE: usize = 10;
const INLINE_TABLE_VAR: usize = 13;
/// Its second scan (from 0x10000, as the allocators'): jbe use ; nop ; loop: cmp [ecx+esi*4],0 ; je found ; inc esi ;
/// cmp esi,eax ; jb loop ; use: push esi ; push
const INLINE_TAIL: usize = 0x2C;
const INLINE_TAIL_CODE: &str = "0E 0F 1F 00 83 3C B1 00 74 05 46 3B F0 72 F5 56 68";
const INLINE_USE: usize = 0x3B;
/// The global reset: push ebp ; mov ebp,esp ; push ecx ; mov ecx,[count] ; push esi ; mov esi,[free] ; push edi ;
/// xor edi,edi. It frees the tables whose count globals it mentions.
const RESET: &str = "55 8B EC 51 8B 0D ?? ?? ?? ?? 56 8B 35 ?? ?? ?? ?? 57 33 FF";
const RESET_LEN: usize = 0x800;
/// Its moved entry (push ebp ; mov ebp,esp ; push ecx ; mov ecx,[imm32]): position-independent, but the operand is
/// relocated, so `Image::hook`'s check would refuse it.
const RESET_ENTRY: usize = 10;

/// A hinted table: the globals holding its count and its address.
#[derive(Clone, Copy, PartialEq)]
struct Table {
    count: usize,
    table: usize,
}

/// The hinted tables, by id.
static TABLES: Installed<Vec<Table>> = Installed::new();
static HINTS: [AtomicU32; MAX_TABLES] = [const { AtomicU32::new(0) }; MAX_TABLES];
static CALLS: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// BYOND's search: the first null slot from `start`, jumping from 0xFFFF to 0x10000.
///
/// # Safety
/// `table` holds `count` slots.
unsafe fn first_free(table: usize, count: u32, start: u32) -> u32 {
    // SAFETY: below `count` (caller).
    let used = |i: u32| unsafe { live_u32(table + 4 * i as usize) } != 0;
    let mut i = start;
    while i < count && used(i) {
        i += 1;
    }
    if i == RESERVED {
        i = RESERVED + 1;
        while i < count && used(i) {
            i += 1;
        }
    }
    i
}

/// The slot table `id`'s allocator hands out; the caller fills it.
extern "C" fn alloc_index(id: usize) -> u32 {
    let t = TABLES.get()[id];
    // SAFETY: the table's globals (install); the reads stay below its count.
    unsafe {
        let table = live_u32(t.table) as usize;
        let count = live_u32(t.count);
        let mut h = HINTS[id].load(Relaxed);
        if h > count || (h > 0 && h - 1 != RESERVED && live_u32(table + 4 * (h - 1) as usize) == 0) {
            h = 0;
        }
        let r = first_free(table, count, h);
        CALLS.bump();
        if verify() && first_free(table, count, 0) != r {
            MISMATCHES.bump();
        }
        HINTS[id].store(r + 1, Relaxed);
        r
    }
}

/// Runs before the global reset, which frees every hinted table.
extern "C" fn reset_hints() {
    for h in &HINTS {
        h.store(0, Relaxed);
    }
}

/// Table `id`'s slot into esi (`push esi` first for an allocator, which saves it there), then `jmp back`.
fn alloc_stub(push_esi: bool, id: usize, back: usize) -> Result<usize> {
    let mut stub = Stub::new();
    if push_esi {
        stub.bytes(&[0x56]); // push esi
    }
    stub.bytes(&[0x6A, id as u8]) // push id
        .call(alloc_index as extern "C" fn(usize) -> u32) // call alloc_index
        .bytes(&[0x83, 0xC4, 0x04]) // add esp,4
        .bytes(&[0x8B, 0xF0]) // mov esi,eax
        .jmp(back) // jmp back
        .exec()
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (allocs, args) = (core.find_all(ALLOC)?, core.find_all(ARGS)?);
    ensure!(args.len() <= 1, "argument slot allocator: expected 0-1, found {}", args.len());
    // (start, where its scan ends)
    let found: Vec<(usize, usize)> =
        allocs.iter().map(|&a| (a, a + ALLOC_USE)).chain(args.iter().map(|&a| (a, a + ARGS_USE))).collect();
    ensure!((1..=MAX_TABLES).contains(&found.len()), "slot allocators: expected 1-{MAX_TABLES}, found {}", found.len());
    let clears =
        |reset: usize, a: usize| -> Result<bool> { core.mentions(reset, RESET_LEN, core.ptr_at(a + COUNT_VAR)?) };
    let reset =
        core.probe_unique(RESET, "table reset", |r| found.iter().any(|&(a, _)| clears(r, a).unwrap_or(false)))?;

    let mut parts = cx.parts();
    let (mut tables, mut sites) = (Vec::new(), Vec::new());
    for &(a, back) in &found {
        if !clears(reset, a)? {
            parts.skip(format!("allocator at {:#x} (table not cleared by the reset)", a - core.base));
            continue;
        }
        sites.push((a, alloc_stub(true, tables.len(), back)?));
        tables.push(Table { count: core.ptr_at(a + COUNT_VAR)?, table: core.ptr_at(a + TABLE_VAR)? });
    }
    let allocators = sites.len();
    // Each pproc copy shares its table's hint.
    for &m in core.find_all(INLINE)?.iter() {
        let t = Table { count: core.ptr_at(m + COUNT_VAR)?, table: core.ptr_at(m + INLINE_TABLE_VAR)? };
        let Some(id) = tables.iter().position(|&h| h == t) else {
            parts.skip(format!("inlined scan at {:#x} (its table has no hinted allocator)", m - core.base));
            continue;
        };
        if core.rel8(m + INLINE_JE_USE)? != m + INLINE_USE || !core.is_at(m + INLINE_TAIL, INLINE_TAIL_CODE)? {
            parts.skip(format!("inlined scan at {:#x} (unexpected continuation)", m - core.base));
            continue;
        }
        sites.push((m, alloc_stub(false, id, m + INLINE_USE)?));
    }
    let inlined = sites.len() - allocators;

    let reset_original = trampoline(core.bytes(reset, RESET_ENTRY)?, reset + RESET_ENTRY)?;
    let reset_wrapper = Stub::new().call(reset_hints as extern "C" fn()).jmp(reset_original).exec()?;
    core.jump(reset, RESET_ENTRY, reset_wrapper)?;
    for (at, stub) in sites {
        core.jump(at, 5, stub)?;
    }
    TABLES.commit(cx, tables);
    // The writes above are the feature's own, listed as one part.
    parts.part(|| Ok(format!("{allocators} allocators, {inlined} inlined")));
    parts.finish()
}

pub(super) fn print_stats() {
    eprintln!("dmfast: slot hints calls={CALLS} verify={} mismatches={MISMATCHES}", verify());
}
