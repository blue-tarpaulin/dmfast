//! `DMFAST_VERIFY=1`: every intern also walks BYOND's tree, whose answer is used; the index's differing is a mismatch.
//! The intern then links a new entry where the walk ended, so the tree stays BYOND's.

use std::cmp::Ordering;

use super::{Found, Index, LEFT, RIGHT, string};
use crate::base::code::live_u32;

/// BYOND's walk from slot 0's entry: the entry holding `key` (counting a mismatch if the index's `found` differs), and
/// the empty link where the original stores a new entry (`None`: an empty tree, or a hit).
///
/// # Safety
/// The table's globals (install); no one changes the tree meanwhile.
pub(super) unsafe fn walk(
    f: &Found,
    ix: &mut Index,
    key: &[u8],
    found: Option<usize>,
) -> (Option<usize>, Option<usize>) {
    // SAFETY: live entries and their links (caller).
    let (real, link) = unsafe {
        let (mut node, mut link) = (f.entry(0), None);
        loop {
            let Some(e) = node else { break (None, link) };
            let l = match key.cmp(string(e)) {
                Ordering::Equal => break (Some(e), None),
                Ordering::Less => e + LEFT,
                Ordering::Greater => e + RIGHT,
            };
            link = Some(l);
            node = Some(live_u32(l) as usize).filter(|&n| n != 0);
        }
    };
    if real != found {
        ix.mismatches += 1;
    }
    (real, link)
}
