//! `varlist`, Windows: byondcore's var list merge (cdecl) and the var list pass that calls it are hooked at entry.

use std::sync::Mutex;

use super::verify;
use crate::base::code::Foreign;
use crate::base::driver::{self, Counter};
use crate::base::feature::Ctx;
use crate::base::hash::{self, Map};
use crate::base::sync::lock;
use crate::base::{Result, ensure};

/// BYOND's list.
#[repr(C)]
struct List {
    count: u16,
    _pad: u16,
    refcount: i32,
    data: *const u32,
}

/// A list the merge returned, as it was then.
#[derive(Clone, Copy)]
struct Returned {
    id: u32,
    data: usize,
    count: u16,
}

type Merge = unsafe extern "C" fn(*const List, *const List) -> u32;

static ORIGINAL: Foreign<Merge> = Foreign::new();
static LOOKUP: Foreign<unsafe extern "C" fn(u32) -> *mut List> = Foreign::new();
static PASS: Foreign<unsafe extern "C" fn()> = Foreign::new();
/// The non-empty lists the merge returned during this pass, by address.
static RETURNED: Mutex<Map<usize, Returned>> = Mutex::new(hash::map());
static CALLS: Counter = Counter::new();
static SHORTCUTS: Counter = Counter::new();

/// The merge (`own`, `parent`): push ebp ; mov ebp,esp ; mov eax,[ebp+8] ; sub esp,14h ; test eax,eax ; je +5 ;
/// movzx edx,word [eax] ; jmp +2 ; xor edx,edx ; push ebx ; push esi ; mov esi,[ebp+0Ch] ; mov [ebp-0Ch],edx ;
/// movzx eax,dx ; push edi
const MERGE: &str = "55 8B EC 8B 45 08 83 EC 14 85 C0 74 05 0F B7 10 EB 02 33 D2 53 56 8B 75 0C 89 55 F4 0F B7 C2 57";
/// The hooked bytes: push ebp ; mov ebp,esp ; mov eax,[ebp+8].
const ENTRY: usize = 6;
/// The merge's tail: push ebx ; push edi ; call intern ; push edi ; mov esi,eax ; call [free] ; push esi ;
/// call lookup ; add esp,10h ; test eax,eax ; je +3 ; inc dword [eax+4] ; pop edi ; mov eax,esi ; pop esi ; pop ebx ;
/// mov esp,ebp ; pop ebp ; ret
const TAIL: usize = 0x266;
const TAIL_CODE: &str = "53 57 E8 ?? ?? ?? ?? 57 8B F0 FF 15 ?? ?? ?? ?? 56 E8 ?? ?? ?? ?? 83 C4 10 85 C0 74 03 FF 40 04 5F 8B C6 5E 5B 8B E5 5D C3";
/// The tail's `call lookup`.
const LOOKUP_CALL: usize = 17;
/// The list lookup: push ebp ; mov ebp,esp ; mov ecx,[ebp+8] ; cmp ecx,[count] ; jb +4 ; xor eax,eax ; pop ebp ; ret ;
/// mov eax,[table] ; mov eax,[eax+ecx*4] ; pop ebp ; ret
const LOOKUP_CODE: &str = "55 8B EC 8B 4D 08 3B 0D ?? ?? ?? ?? 72 04 33 C0 5D C3 A1 ?? ?? ?? ?? 8B 04 88 5D C3";
/// The var list pass (cdecl, once per .dmb load) that merges every type: SEH prologue ; mov esi,[type count] ;
/// xor edi,edi ; test esi,esi ; je ... ; push edi ; call per-type
const PASS_CODE: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 14 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 \
                         8D 45 F4 64 A3 00 00 00 00 8B 35 ?? ?? ?? ?? 33 FF 85 F6 74 4C 57 E8";
/// The hooked bytes: push ebp ; mov ebp,esp ; push -1.
const PASS_ENTRY: usize = 5;
/// The pass's `call per-type`, and the per-type function's `call merge`.
const PER_TYPE_CALL: usize = 0x35;
const MERGE_CALL: usize = 0xE2;

fn lookup(id: u32) -> *mut List {
    // SAFETY: BYOND's list lookup (bounds-checked id), found at install.
    unsafe { LOOKUP.get()(id) }
}

/// `parent`'s id, if `own` is empty and `parent` is a list the merge returned during this pass that the table still
/// holds unchanged.
///
/// # Safety
/// `own` and `parent` are null or live lists.
unsafe fn known(own: *const List, parent: *const List) -> Option<u32> {
    // SAFETY: null or live lists (caller).
    let (own, p) = unsafe { (own.as_ref(), parent.as_ref()?) };
    if own.is_some_and(|o| o.count != 0) {
        return None;
    }
    let r = *lock(&RETURNED).get(&(parent as usize))?;
    (lookup(r.id).cast_const() == parent && p.data as usize == r.data && p.count == r.count).then_some(r.id)
}

/// Note the list the merge returned as `id`.
fn remember(id: u32) {
    // SAFETY: the table's list for `id`, live or null.
    let Some(l) = (unsafe { lookup(id).as_ref() }) else { return };
    if l.count != 0 {
        let r = Returned { id, data: l.data as usize, count: l.count };
        lock(&RETURNED).insert(l as *const List as usize, r);
    }
}

/// # Safety
/// `own` and `parent` are null or live lists (BYOND passes them).
unsafe extern "C" fn merge(own: *const List, parent: *const List) -> u32 {
    CALLS.bump();
    // SAFETY: null or live lists (caller).
    let shortcut = unsafe { known(own, parent) };
    if let Some(id) = shortcut {
        SHORTCUTS.bump();
        if !driver::verify() {
            // SAFETY: `parent` is live (`known`): the reference the merge's intern would have taken.
            unsafe { (*parent.cast_mut()).refcount += 1 };
            return id;
        }
    }
    // SAFETY: BYOND's merge, with its arguments.
    let id = unsafe { ORIGINAL.get()(own, parent) };
    if let Some(mine) = shortcut {
        verify::check(mine, id);
    }
    remember(id);
    id
}

/// A var list pass is starting: lists from an earlier load may have been freed.
unsafe extern "C" fn start_pass() {
    lock(&RETURNED).clear();
    // SAFETY: BYOND's pass, which this replaces.
    unsafe { PASS.get()() }
}

pub(super) fn print_stats() {
    if driver::verify() {
        eprintln!(
            "dmfast: var list merge: {CALLS} calls, {SHORTCUTS} shortcut, verify mismatches={}",
            verify::MISMATCHES
        );
    } else {
        eprintln!("dmfast: var list merge: {CALLS} calls, {SHORTCUTS} shortcut");
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(MERGE, "var list merge")?;
    core.require(f + TAIL, TAIL_CODE, "var list merge: tail")?;
    let lookup_fn = core.follow_call(f + TAIL + LOOKUP_CALL, LOOKUP_CODE, "var list merge: list lookup")?;
    let pass = core.find_unique(PASS_CODE, "var list pass")?;
    let per_type = core.call_target(pass + PER_TYPE_CALL)?;
    ensure!(core.call_target(per_type + MERGE_CALL)? == f, "var list pass: does not call the merge");
    LOOKUP.set(lookup_fn);
    ORIGINAL.set(core.hook(f, ENTRY, merge as Merge)?);
    PASS.set(core.hook(pass, PASS_ENTRY, start_pass as unsafe extern "C" fn())?);
    Ok(format!("at +{:#x}, pass at +{:#x}", f - core.base, pass - core.base))
}
