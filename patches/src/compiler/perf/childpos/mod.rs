//! `childpos`: a node's position among its parent's children, scanned 8 at a time. Name resolution asks for it at
//! every step of a scope walk and twice per forward-reference check of a local var, and BYOND compares one child per
//! iteration (parents reach ~1,400 children).
//!
//! The answer is the stock one: the first child equal to the node, or -1 without a node, a parent, or a match. The
//! function is replaced whole; its signature covers all of it.

use std::arch::x86::{__m128i, _mm_cmpeq_epi32, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi32};

use crate::base::code::{live_u32, read};
use crate::base::feature::{Feature, Hosts, install};
use crate::compiler::layout::{CHILDREN, COUNT, PARENT};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static CHILDPOS: Feature = Feature::perf(
    Hosts::COMPILER,
    "childpos",
    "child position scan",
    install!(windows: windows::install, linux: linux::install),
);

/// `node`'s position in its parent's children, or -1.
extern "C" fn position(node: usize) -> i32 {
    if node == 0 {
        return -1;
    }
    // SAFETY: BYOND passes a live node; its parent and the parent's children array are live with it.
    unsafe {
        let parent = live_u32(node + PARENT) as usize;
        if parent == 0 {
            return -1;
        }
        let count = usize::from(read::<u16>(parent + COUNT));
        let children = live_u32(parent + CHILDREN) as *const u32;
        find(children, count, node as u32).map_or(-1, |i| i as i32)
    }
}

/// The first `i < count` with `children[i] == node`.
///
/// # Safety
/// `children` holds `count` readable dwords.
unsafe fn find(children: *const u32, count: usize, node: u32) -> Option<usize> {
    let mut i = 0;
    // SAFETY: every load is of 4 dwords below `count` (caller); SSE2 is the baseline.
    unsafe {
        let want = _mm_set1_epi32(node as i32);
        let at = |i: usize| _mm_cmpeq_epi32(_mm_loadu_si128(children.add(i).cast::<__m128i>()), want);
        while i + 8 <= count {
            let (a, b) = (at(i), at(i + 4));
            if _mm_movemask_epi8(_mm_or_si128(a, b)) != 0 {
                let m = (_mm_movemask_epi8(a) as u32) | ((_mm_movemask_epi8(b) as u32) << 16);
                return Some(i + m.trailing_zeros() as usize / 4);
            }
            i += 8;
        }
        (i..count).find(|&i| *children.add(i) == node)
    }
}

#[cfg(test)]
mod tests {
    use super::find;

    #[test]
    fn first_match_in_every_lane_and_tail() {
        for count in 0..40 {
            let v: Vec<u32> = (0..count as u32).map(|i| i % 13).collect();
            for want in 0..14 {
                // SAFETY: `v` holds `count` dwords.
                let got = unsafe { find(v.as_ptr(), count, want) };
                assert_eq!(got, v.iter().position(|&x| x == want), "count {count} want {want}");
            }
        }
    }
}
