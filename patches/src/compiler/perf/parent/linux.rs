//! `parent`, Linux: the lookup (cdecl) jumps to [`parent_stub`], which answers it or tail-calls its `follow` with the
//! `..` child. A hit also needs the same last child and the cached child to still be a `..` link; the pool free
//! goes through [`free_stub`], which drops the freed object's entry.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::{Cache, MIN_CHILDREN, NONE, SLOT_BITS, TYPE_PARENT_LINK, scan, slot, tag, verify};
use crate::base::Result;
use crate::base::code::{Addr, live_u32, read};
use crate::base::feature::Ctx;
use crate::base::sync::Owned;
use crate::compiler::layout::{CHILDREN, COUNT, PARENT};

/// The whole lookup: count-1 ; loop: child = children[i--] ; cmp [child],45h ; je found ; i != -1 ;
/// return [node+PARENT] ... found: [esp+4] = child ; jmp follow
const LOOKUP: &str = "8B 4C 24 04 0F B7 41 1E 83 E8 01 EB 11 8D 76 00 8B 51 18 8B 14 82 83 E8 01 83 3A 45 74 12 \
                      83 F8 FF 75 ED 8B 41 14 C3 ?? ?? ?? ?? ?? ?? ?? ?? ?? 89 54 24 04 E9";
const FOLLOW_JUMP: usize = 0x34;
/// The jump to [`parent_stub`] (its first two instructions; nothing jumps into them): finalpath matches the lookup
/// after these.
const ENTRY: usize = 8;
/// `follow(child)`: push ebx ; xor edx,edx ; sub esp,8 ; mov eax,[esp+10h] (the child)
const FOLLOW: &[u8] = &[0x53, 0x31, 0xD2, 0x83, 0xEC, 0x08, 0x8B, 0x44];
/// The pool free (cdecl: block, object): push edi ; push esi ; push ebx ; mov eax,[esp+14h] ; mov ebx,[esp+10h] ;
/// test the in-use bit ; clear it ; push the object on the block's free list ...
const FREE: &str = "57 56 53 8B 44 24 14 8B 5C 24 10 80 78 2F 00 0F 89 ?? ?? ?? ?? 80 60 2F 7F 8B 53 0C 89 10 \
                    83 6B 10 01 89 43 0C";
/// Its hooked entry: push edi ; push esi ; push ebx ; mov eax,[esp+14h]
const FREE_ENTRY: usize = 7;

/// The rest of the original lookup, from its `..` child.
static FOLLOW_AT: Addr = Addr::new();
/// The original pool free.
static ORIGINAL_FREE: Addr = Addr::new();

#[derive(Default)]
pub(super) struct Entry {
    /// 0 = empty; cleared from any thread by [`evict`].
    node: AtomicU32,
    children: u32,
    last: u32,
    count: u16,
    idx: u16,
}

/// Other threads bypass the cache and only clear `node` (atomic) in [`evict`].
pub(super) static CACHE: Owned<Cache<Entry>> = Owned::new();

/// The lookup's answer: (parent, 0) to return it, or (child, 1) to continue with `follow(child)`.
///
/// # Safety
/// `node` is a live node with live children (the stub passes the lookup's node).
unsafe extern "C" fn resolve(node: usize) -> u64 {
    // SAFETY: a live node (caller): its fields and children.
    unsafe {
        let count = read::<u16>(node + COUNT);
        let children = live_u32(node + CHILDREN) as usize;
        let idx = if count < MIN_CHILDREN {
            scan(children, count)
        } else if let Some(cache) = CACHE.get() {
            let last = live_u32(children + 4 * (count as usize - 1));
            let e = &mut cache.slots[slot(node as u32)];
            let hit = e.node.load(Relaxed) == node as u32
                && e.children == children as u32
                && e.count == count
                && e.last == last
                && (e.idx == NONE || tag(children, e.idx) == TYPE_PARENT_LINK);
            if hit {
                cache.hits += 1;
                verify::recheck(&mut e.idx, children, count);
                e.idx
            } else {
                cache.misses += 1;
                let idx = scan(children, count);
                e.node.store(node as u32, Relaxed);
                (e.children, e.last, e.count, e.idx) = (children as u32, last, count, idx);
                idx
            }
        } else {
            scan(children, count)
        };
        if idx == NONE {
            u64::from(live_u32(node + PARENT))
        } else {
            u64::from(live_u32(children + 4 * idx as usize)) | 1 << 32
        }
    }
}

/// Replaces the lookup (cdecl, [esp+4] = node). `follow` may throw, so it is tail-called from here.
///
/// # Safety
/// Only jumped to from the lookup's entry.
#[unsafe(naked)]
unsafe extern "C" fn parent_stub() {
    naked_asm!(
        // Entered by `call`: esp+4 is 16-aligned. Keep the call into Rust aligned.
        "sub esp, 8",
        "push dword ptr [esp + 12]",
        "call {resolve}",
        "add esp, 12",
        "test edx, edx",
        "jnz 2f",
        "ret",
        "2:",
        "mov dword ptr [esp + 4], eax",
        "jmp dword ptr [{follow}]",
        resolve = sym resolve,
        follow = sym FOLLOW_AT,
    )
}

/// Drop the entry of a pool object being freed (any pool, any thread).
extern "C" fn evict(obj: u32) {
    // SAFETY: only the atomic `node` fields are touched.
    if let Some(cache) = unsafe { CACHE.shared() } {
        let _ = cache.slots[slot(obj)].node.compare_exchange(obj, 0, Relaxed, Relaxed);
    }
}

/// Hooks the pool free (cdecl, [esp+8] = object): evict, then the original.
///
/// # Safety
/// Only jumped to from the pool free's entry.
#[unsafe(naked)]
unsafe extern "C" fn free_stub() {
    naked_asm!(
        "sub esp, 8",
        "push dword ptr [esp + 16]",
        "call {evict}",
        "add esp, 12",
        "jmp dword ptr [{original}]",
        evict = sym evict,
        original = sym ORIGINAL_FREE,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(LOOKUP, "parent lookup")?;
    let follow = core.branch_at(f + FOLLOW_JUMP)?.target;
    core.expect(follow, FOLLOW, "unexpected code at the parent follow function")?;
    let free = core.find_unique(FREE, "pool free")?;
    FOLLOW_AT.set(follow);
    CACHE.set(Cache { slots: (0..1 << SLOT_BITS).map(|_| Entry::default()).collect(), hits: 0, misses: 0 })?;
    ORIGINAL_FREE.set(core.hook(free, FREE_ENTRY, free_stub as unsafe extern "C" fn())?);
    core.jump(f, ENTRY, parent_stub as unsafe extern "C" fn())?;
    Ok(core.report(&[f]))
}
