//! `treeskip`, Windows: byondcore's send calls [`unseen`] and [`placed`] for its two collector calls, and each flag
//! walk's list load calls [`stub`].

use std::arch::naked_asm;

use super::{FIRST, PLACED, scan, walk};
use crate::base::code::{Foreign, read};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The send's start (frame of 428h, [ebp-408h] = record tree esi), through the unseen-record walk: push the
/// predicate (+63h), call the list collector (+81h) with the tree, the 250-slot array [ebp-3ECh], and its free count
/// [ebp-3FCh] = 0FAh.
const SEND: &str = "55 8B EC 81 EC 28 04 00 00 A1 ?? ?? ?? ?? 33 C5 89 45 FC 0F B7 45 08 53 8B 5D 10 56 8B 75 0C 57 8B 7D \
                    14 50 89 85 E8 FB FF FF 89 B5 F8 FB FF FF 89 BD FC FB FF FF C7 85 F0 FB FF FF 00 00 00 00 E8 ?? ?? ?? \
                    ?? 83 C4 04 89 85 EC FB FF FF 85 C0 0F 84 ?? ?? ?? ?? 8B CE C7 07 00 00 00 00 E8 ?? ?? ?? ?? 68 ?? ?? \
                    ?? ?? 8D 85 04 FC FF FF C7 85 04 FC FF FF FA 00 00 00 50 8D 85 14 FC FF FF 50 56 E8";
const UNSEEN_PUSH: usize = 0x63;
const UNSEEN_CALL: usize = 0x81;
/// The send's length (to the `ret` and its padding).
const LEN: usize = 0x105A;
/// The flag-2 walk: push the predicate, call the collector (+23h) with tree [ebp-408h].
const PLACED_WALK: &str =
    "68 ?? ?? ?? ?? 8D 85 04 FC FF FF C7 85 04 FC FF FF FA 00 00 00 50 8D 85 14 FC FF FF 50 FF B5 F8 FB FF FF E8";
const PLACED_CALL: usize = 0x23;
/// What the flag-2 walk does to its records: flags & 0FFDDh | 4.
const PLACED_SETS: &str = "8B 0F BA DD FF 00 00 66 8B 41 4C 66 23 C2 66 83 C8 04";
/// The predicates: not seen (flags bit 4 clear), and flag 2.
const UNSEEN_PRED: &str = "55 8B EC 8B 45 08 66 8B 40 4C 66 C1 E8 04 66 F7 D0 66 83 E0 01 5D C3";
const PLACED_PRED: &str = "55 8B EC 8B 45 08 66 8B 40 4C 66 83 E0 02 5D C3";
/// The list collector (cdecl tree, array, free count, predicate): each record of the list [tree+4] (next +0Ch) the
/// predicate holds for goes into the array from the top while the count is above 0; nothing for an empty list.
const COLLECT: &str = "55 8B EC 56 8B 75 08 8B 76 04 85 F6 74 3C 53 8B 5D 10 57 8B 7D 0C 56 FF 55 14 83 C4 04 66 85 C0 74 \
                       1F 0F B7 03 66 85 C0 74 0C 48 66 89 03 0F B7 C0 89 34 87 EB 0B 81 7E 2C FF FF 00 00 75 02 89 37 8B \
                       76 0C 85 F6 75 CE 5F 5B 5E 5D C3";
/// Each flag walk: its stub, the code before its list load (eax = tree [ebp-408h], edi = 0FAh; the flags 100h, 400h,
/// and 800h into edx), the load `mov ecx,[eax+4] ; test ecx,ecx` (replaced), and after it the `je` to the walk's end
/// and its loop's test of the flag.
const LOAD: &str = "8B 48 04 85 C9";
const TREE: &str = "8B 85 F8 FB FF FF BF FA 00 00 00 89 BD 00 FC FF FF";
const IN_EDX: &str = "0F 84 ?? ?? ?? ?? 66 85 51 4C 74";
const WALKS: [(Stub, &str, &str); 8] = [
    (stub::<0x004>, TREE, "74 ?? F6 41 4C 04 74"),
    (stub::<0x001>, TREE, "0F 84 ?? ?? ?? ?? F6 41 4C 01 0F 84"),
    (stub::<0x008>, TREE, "0F 84 ?? ?? ?? ?? F6 41 4C 08 74"),
    (stub::<0x040>, TREE, "0F 84 ?? ?? ?? ?? F6 41 4C 40 74"),
    (stub::<0x080>, TREE, "0F 84 ?? ?? ?? ?? F6 41 4C 80 74"),
    (stub::<0x100>, "BA 00 01 00 00 75 ?? 8B 85 F8 FB FF FF BF FA 00 00 00 89 BD 00 FC FF FF", IN_EDX),
    (stub::<0x400>, "BA 00 04 00 00 75 ?? 8B 85 F8 FB FF FF BF FA 00 00 00 89 BD 00 FC FF FF", IN_EDX),
    (stub::<0x800>, "BA 00 08 00 00 75 ?? 8B 85 F8 FB FF FF BF FA 00 00 00", IN_EDX),
];

type Stub = unsafe extern "C" fn();
type CollectFn = unsafe extern "C" fn(usize, usize, usize, usize);

static COLLECTOR: Foreign<CollectFn> = Foreign::new();

/// The collector from `start` (the collector reads only a tree's first record), unless it's none.
///
/// # Safety
/// As the collector; `start` is a record of the list or 0.
unsafe fn collect(start: usize, array: usize, free: usize, pred: usize) {
    if start != 0 {
        let mut tree = [0usize; 2];
        tree[FIRST / size_of::<usize>()] = start;
        // SAFETY: the collector (caller), on the list from `start`.
        unsafe { COLLECTOR.get()(tree.as_ptr() as usize, array, free, pred) }
    }
}

/// The unseen-record walk's collector call: [`scan`], then the collector from the first unseen record.
///
/// # Safety
/// As the collector, on a client's record tree only this thread uses.
unsafe extern "C" fn unseen(tree: usize, array: usize, free: usize, pred: usize) {
    // SAFETY: a live tree (caller).
    unsafe { collect(scan(read(tree + FIRST)), array, free, pred) }
}

/// The flag-2 walk's collector call: the collector from the first record with flag 2.
///
/// # Safety
/// As [`unseen`], after it on the same tree.
unsafe extern "C" fn placed(tree: usize, array: usize, free: usize, pred: usize) {
    // SAFETY: a live tree (caller).
    unsafe { collect(walk(read(tree + FIRST), PLACED.into()), array, free, pred) }
}

/// A flag walk's list load (eax = tree): ecx = its start from [`walk`], and `test ecx,ecx`; eax and edx kept.
///
/// # Safety
/// Only called from a flag walk's list load.
#[unsafe(naked)]
unsafe extern "C" fn stub<const FLAG: u32>() {
    naked_asm!(
        "push eax",
        "push edx",
        "push {flag}",
        "push dword ptr [eax + {first}]",
        "call {walk}",
        "add esp, 8",
        "mov ecx, eax",
        "pop edx",
        "pop eax",
        "test ecx, ecx",
        "ret",
        flag = const FLAG,
        first = const FIRST,
        walk = sym walk,
    )
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(SEND, "record change send")?;
    let collector = core.follow_call(f + UNSEEN_CALL, COLLECT, "record list collector")?;
    core.require(core.ptr_at(f + UNSEEN_PUSH + 1)?, UNSEEN_PRED, "unseen-record predicate")?;
    let p = core.find_unique_in(f, LEN, PLACED_WALK, "flag-2 walk")?;
    ensure!(core.call_target(p + PLACED_CALL)? == collector, "flag-2 walk: not the list collector");
    core.require(core.ptr_at(p + 1)?, PLACED_PRED, "flag-2 predicate")?;
    core.find_unique_in(f, LEN, PLACED_SETS, "flag-2 walk's flags")?;
    let mut sites = vec![f + UNSEEN_CALL, p + PLACED_CALL];
    for (stub, before, after) in WALKS {
        let sig = format!("{before} {LOAD} {after}");
        let at = core.find_unique_in(f, LEN, &sig, "flag walk")? + before.split_whitespace().count();
        core.untouched(&[(at, 5)])?;
        core.call(at, 5, stub)?;
        sites.push(at);
    }
    COLLECTOR.set(collector);
    core.retarget(f + UNSEEN_CALL + 1, unseen as CollectFn)?;
    core.retarget(p + PLACED_CALL + 1, placed as CollectFn)?;
    Ok(core.report(&sites))
}
