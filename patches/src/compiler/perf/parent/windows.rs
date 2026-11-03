//! `parent`, Windows: the lookup (fastcall, ecx = node) jumps to [`parent_stub`], which answers it. The child
//! append's error paths reset the count and overwrite slots in place, but only after the compile has failed. Every
//! pool allocator returns through [`evict_stub`], which drops the new object's entry; a never-cleared filter bit per
//! address hash is checked first, so most allocations skip the table.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::{Cache, MIN_CHILDREN, NONE, SLOT_BITS, hash, scan, slot, verify};
use crate::base::code::{Addr, Foreign, Stub, live_u32, read, trampoline};
use crate::base::feature::Ctx;
use crate::base::sync::Owned;
use crate::base::{Result, ensure};
use crate::compiler::layout::{CHILDREN, COUNT, ID, KIND, PARENT};

/// The whole lookup up to its call of the path lookup: count ; scan children backwards for id 45h ; else return
/// [node+PARENT] ; follow one-child pass-through nodes ; push [child+8] ; push [child+PARENT] ; call path_lookup
const LOOKUP: &str = "0F B7 51 22 56 57 85 D2 74 16 8B 79 1C 8D 34 97 8B 46 FC 8D 76 FC 4A 83 38 45 74 0A 85 D2 75 F0 \
                      8B 41 18 5F 5E C3 8B 04 97 85 C0 74 38 83 38 00 75 17 80 78 04 00 75 11 66 83 78 22 01 75 0A \
                      8B 48 1C 8B 01 83 38 00 74 E9 8B 48 18 85 C9 74 15 80 78 04 04 75 0F FF 70 08 51 E8";
const PATH_CALL: usize = 0x5A;
/// The jump to [`parent_stub`] (movzx edx,word [ecx+22h] ; push esi): finalpath matches the lookup after these.
const ENTRY: usize = 5;
/// A pool allocator (one copy per pool, nodes among them): SEH prologue, then mov edi,ecx ; ... ;
/// mov esi,[edi+4] ; ... ; call take. Evicting on all of them is harmless.
const POOL: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 10 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 \
                    8D 45 F4 64 A3 00 00 00 00 89 65 F0 8B F9 89 7D E4 8B 77 04 89 75 E8 85 F6 74 ?? 8B CE \
                    C7 45 FC 00 00 00 00 E8 ?? ?? ?? ?? 83 7E 04 00 8B D8";
/// Its moved entry (push ebp ; mov ebp,esp ; push -1 ; push handler): position-independent, but the handler's
/// address is relocated, so `Image::hook`'s check would refuse it.
const POOL_ENTRY: usize = 10;
const MAX_POOLS: usize = 8;

/// The path lookup (cdecl: parent link, child+8).
type PathLookup = unsafe extern "C" fn(u32, u32) -> u32;
static PATH_LOOKUP: Foreign<PathLookup> = Foreign::new();
/// The table's address, for [`evict_stub`].
static SLOTS: Addr = Addr::new();

/// 12 bytes: [`evict_stub`] indexes the table by hand.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(super) struct Entry {
    node: u32,
    children: u32,
    count: u16,
    idx: u16,
}

pub(super) static CACHE: Owned<Cache<Entry>> = Owned::new();

/// Bits in the "may be cached" filter (32 KB): the slot index plus one bit of the same hash.
const MARK_BITS: u32 = SLOT_BITS + 1;
/// The filter. Written only by the cache's owner, read by [`evict_stub`] on any thread.
static MARKS: [AtomicU32; 1 << (MARK_BITS - 5)] = [const { AtomicU32::new(0) }; 1 << (MARK_BITS - 5)];

/// Before an entry for `node` is stored: from now on its evictions reach the table.
fn mark(node: u32) {
    let b = hash(node) >> (32 - MARK_BITS);
    let w = &MARKS[(b >> 5) as usize];
    // Single writer: no read-modify-write needed.
    w.store(w.load(Relaxed) | 1 << (b & 31), Relaxed);
}

/// The lookup's answer: (value, 0) to return `value`, or (child, 1) to return `path_lookup(child's parent, child+8)`.
///
/// # Safety
/// `node` is a live node with live children (the stub passes the lookup's node).
unsafe extern "C" fn resolve(node: usize) -> u64 {
    // SAFETY: a live node (caller): its fields and children.
    unsafe {
        let count = read::<u16>(node + COUNT);
        let children = live_u32(node + CHILDREN) as usize;
        let idx = if count == 0 {
            NONE
        } else if count < MIN_CHILDREN {
            scan(children, count)
        } else if let Some(cache) = CACHE.get() {
            let e = &mut cache.slots[slot(node as u32)];
            if e.node == node as u32 && e.children == children as u32 && e.count == count {
                cache.hits += 1;
                verify::recheck(&mut e.idx, children, count);
                e.idx
            } else {
                cache.misses += 1;
                let idx = scan(children, count);
                mark(node as u32);
                *e = Entry { node: node as u32, children: children as u32, count, idx };
                idx
            }
        } else {
            scan(children, count)
        };
        if idx == NONE {
            return u64::from(live_u32(node + PARENT));
        }
        // Follow single-child pass-through nodes (id 0, kind 0, one child).
        let mut c = live_u32(children + 4 * idx as usize) as usize;
        if c == 0 {
            return 0;
        }
        while live_u32(c + ID) == 0 && read::<u8>(c + KIND) == 0 && read::<u16>(c + COUNT) == 1 {
            c = live_u32(live_u32(c + CHILDREN) as usize) as usize;
        }
        if live_u32(c + PARENT) == 0 || read::<u8>(c + KIND) != 4 {
            return 0;
        }
        c as u64 | 1 << 32
    }
}

/// Replaces the lookup (fastcall, ecx = node). The path lookup may throw, so it is called from here, not from Rust.
///
/// # Safety
/// Only jumped to from the lookup's entry.
#[unsafe(naked)]
unsafe extern "C" fn parent_stub() {
    naked_asm!(
        "push ecx",
        "call {resolve}",
        "add esp, 4",
        "test edx, edx",
        "jnz 2f",
        "ret",
        "2:",
        "push dword ptr [eax + 8]",
        "push dword ptr [eax + {parent}]",
        "call dword ptr [{path_lookup}]",
        "add esp, 8",
        "ret",
        resolve = sym resolve,
        path_lookup = sym PATH_LOOKUP,
        parent = const PARENT,
    )
}

/// Tail of every pool allocator wrapper (eax = the new object; ecx and edx are free): if the address may be cached,
/// clear its slot. Clearing a slot is harmless from any thread.
///
/// # Safety
/// Only jumped to from the pool allocator wrappers.
#[unsafe(naked)]
unsafe extern "C" fn evict_stub() {
    const _: () = assert!(size_of::<Entry>() == 12);
    naked_asm!(
        "imul edx, eax, 0x9E3779B1",
        "shr edx, {mark_shift}",
        "mov ecx, edx",
        "shr ecx, 5",
        "mov ecx, dword ptr [{marks} + ecx*4]",
        "bt ecx, edx",
        "jnc 2f",
        // slot = mark index >> 1; entry at slots + 12*slot
        "shr edx, 1",
        "lea edx, [edx + edx*2]",
        "mov ecx, dword ptr [{slots}]",
        "cmp dword ptr [ecx + edx*4], eax",
        "jne 2f",
        "mov dword ptr [ecx + edx*4], 0",
        "2:",
        "ret",
        mark_shift = const 32 - MARK_BITS,
        marks = sym MARKS,
        slots = sym SLOTS,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(LOOKUP, "parent lookup")?;
    let path_lookup = core.call_target(f + PATH_CALL)?;
    let pools = core.find_all(POOL)?;
    ensure!((1..=MAX_POOLS).contains(&pools.len()), "pool allocators: expected 1-{MAX_POOLS}, found {}", pools.len());
    let slots = vec![Entry::default(); 1 << SLOT_BITS].into_boxed_slice();
    SLOTS.set(slots.as_ptr() as usize);
    // Each pool allocator: call its moved entry and the rest, then evict.
    for &p in &pools {
        let original = trampoline(core.bytes(p, POOL_ENTRY)?, p + POOL_ENTRY)?;
        let wrapper = Stub::new().call(original).jmp(evict_stub as unsafe extern "C" fn()).exec()?;
        core.jump(p, POOL_ENTRY, wrapper)?;
    }
    PATH_LOOKUP.set(path_lookup);
    CACHE.set(Cache { slots, hits: 0, misses: 0 })?;
    core.jump(f, ENTRY, parent_stub as unsafe extern "C" fn())?;
    Ok(format!("{} ({} pool allocators)", core.report(&[f]), pools.len()))
}
