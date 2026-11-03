//! `DMFAST_VERIFY=1` (Windows): the pass function is hooked by [`verify_visit`], which runs it on every leaf the skip
//! would return from and counts changes to the node, the progress byte, or the finished counter.
//!
//! Stock paths that do touch a finished leaf, none seen on one in practice: kind 6 is converted to kind 4 (clearing
//! both bits) before any flag test, and only a setter clearing both bits makes kind 6; mode 25 sets the progress byte
//! for a kind outside 1..6; mode 7 reports an error only while the node has a line number.

use crate::base::code::{Foreign, read, write};
use crate::base::driver::Counter;
use crate::base::sync::Installed;
use crate::compiler::layout::KIND;

/// Node bytes compared (the node's fields end at +30h).
const NODE: usize = 0x30;

/// What install found: the finished test's fields and BYOND's progress byte and finished counter.
pub(super) struct Found {
    pub flags: usize,
    pub bit: u8,
    pub count: usize,
    pub progress: usize,
    pub finished: usize,
}

pub(super) static FOUND: Installed<Found> = Installed::new();
/// The original pass function (cdecl: node, mode).
pub(super) static ORIGINAL: Foreign<extern "C" fn(usize, u32) -> u32> = Foreign::new();
static CHECKED: Counter = Counter::new();
static MISMATCHES: Counter = Counter::new();

/// The pass function: a leaf the stub would skip runs the original with the progress byte cleared, then the byte gets
/// back its old value or the new one.
///
/// # Safety
/// `node` is a live node (BYOND passes it).
pub(super) unsafe extern "C" fn verify_visit(node: usize, mode: u32) -> u32 {
    let x = FOUND.get();
    // SAFETY: a live node (caller); ORIGINAL is the pass function, the addresses BYOND's globals, found at install.
    unsafe {
        let original = ORIGINAL.get();
        if read::<u8>(node + x.flags) & x.bit == 0 || read::<u16>(node + x.count) != 0 {
            return original(node, mode);
        }
        let (kind, before, was, done) =
            (read::<u8>(node + KIND), read::<[u8; NODE]>(node), read::<u8>(x.progress), read::<u32>(x.finished));
        write(x.progress, 0u8);
        let r = original(node, mode);
        let set = read::<u8>(x.progress);
        write(x.progress, was | set);
        CHECKED.bump();
        if (set != 0 || read::<u32>(x.finished) != done || read::<[u8; NODE]>(node) != before)
            && MISMATCHES.bump_first(20)
        {
            eprintln!("dmfast: leaf skip verify: kind {kind} mode {mode} changed (progress {set})");
        }
        r
    }
}

pub(super) fn print_stats() {
    if FOUND.is_set() {
        eprintln!("dmfast: leaf skip verify checked={CHECKED} mismatches={MISMATCHES}");
    }
}
