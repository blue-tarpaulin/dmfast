//! Signature and byte searches over byte ranges of a mapped image ([`crate::image::Image`] runs them and decides
//! what is relied on). Matches may overlap, so a count includes every occurrence.

#[cfg(target_arch = "x86")]
use std::arch::x86::{__m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{__m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};

use crate::{Result, bail};

/// A searched range: its address and bytes.
pub(crate) type Hay<'a> = (usize, &'a [u8]);

/// Parses an IDA-style pattern: two-digit hex bytes and `??` wildcards, with at least one fixed byte.
pub(crate) fn parse(ida: &str) -> Result<Vec<Option<u8>>> {
    let pat = ida
        .split_whitespace()
        .map(|t| match t {
            "??" => Ok(None),
            _ if t.len() == 2 && t.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(u8::from_str_radix(t, 16).ok()),
            _ => Err(format!("pattern {ida:?}: bad token {t:?}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pat.iter().all(Option::is_none) {
        bail!("pattern {ida:?}: no fixed bytes");
    }
    Ok(pat)
}

/// (start, length) of the longest run of fixed bytes in `pat`.
fn longest_fixed_run(pat: &[Option<u8>]) -> (usize, usize) {
    let (mut best, mut run) = ((0, 0), (0, 0));
    for (i, p) in pat.iter().enumerate() {
        run = if p.is_some() { (if run.1 == 0 { i } else { run.0 }, run.1 + 1) } else { (i + 1, 0) };
        if run.1 > best.1 {
            best = run;
        }
    }
    best
}

/// All matches of a parsed pattern (`None` = wildcard) in `hays`.
pub(crate) fn find_pattern(hays: &[Hay], pat: &[Option<u8>]) -> Vec<usize> {
    let best = longest_fixed_run(pat);
    let anchor: Vec<u8> = pat[best.0..best.0 + best.1].iter().flatten().copied().collect();
    let mut hits = Vec::new();
    for &(start, hay) in hays {
        for at in occurrences(hay, &anchor) {
            let Some(i) = at.checked_sub(best.0).filter(|i| i + pat.len() <= hay.len()) else { continue };
            if hay[i..i + pat.len()].iter().zip(pat).all(|(b, p)| p.is_none_or(|p| p == *b)) {
                hits.push(start + i);
            }
        }
    }
    hits
}

/// All matches of each parsed pattern in `hays` in one pass, via a 4-byte anchor per pattern and a bitmap of the
/// anchors' first two bytes; patterns without 4 fixed bytes in a row go through [`find_pattern`].
pub(crate) fn find_patterns(hays: &[Hay], pats: &[Vec<Option<u8>>]) -> Vec<Vec<usize>> {
    let mut hits = vec![Vec::new(); pats.len()];
    // Per pattern: (index, anchor offset, anchor bytes), bucketed by the first two bytes.
    // 32-bit words: a 64-bit shift costs several instructions on i686, and this test runs at every position.
    let mut bitmap = [0u32; 2048];
    let mut buckets: rustc_hash::FxHashMap<u16, Vec<(usize, usize, u32)>> = Default::default();
    for (j, pat) in pats.iter().enumerate() {
        let best = longest_fixed_run(pat);
        let anchor: Vec<u8> = pat[best.0..best.0 + best.1.min(4)].iter().flatten().copied().collect();
        let Ok(a) = <[u8; 4]>::try_from(anchor) else {
            hits[j] = find_pattern(hays, pat);
            continue;
        };
        let key = u16::from_le_bytes([a[0], a[1]]);
        bitmap[usize::from(key / 32)] |= 1 << (key % 32);
        buckets.entry(key).or_default().push((j, best.0, u32::from_le_bytes(a)));
    }
    for &(start, hay) in hays {
        let len = hay.len();
        for (i, w) in hay.windows(4).enumerate() {
            let key = u16::from_le_bytes([w[0], w[1]]);
            if bitmap[usize::from(key >> 5)] >> (key & 31) & 1 == 0 {
                continue;
            }
            let word = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            for &(j, at, anchor) in buckets.get(&key).into_iter().flatten() {
                let pat = &pats[j];
                if anchor != word || i < at || i - at + pat.len() > len {
                    continue;
                }
                let from = i - at;
                if hay[from..from + pat.len()].iter().zip(pat).all(|(b, p)| p.is_none_or(|p| p == *b)) {
                    hits[j].push(start + from);
                }
            }
        }
    }
    hits
}

/// All occurrences of `needle` in `hays`.
pub(crate) fn find_bytes(hays: &[Hay], needle: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    for &(start, hay) in hays {
        for i in occurrences(hay, needle) {
            hits.push(start + i);
        }
    }
    hits
}

/// Addresses of every `call rel32` (`E8`) to `target` in `hays`.
pub(crate) fn find_calls(hays: &[Hay], target: usize) -> Vec<usize> {
    calls(hays).filter(|&(_, to)| to == target).map(|(at, _)| at).collect()
}

/// Every `call rel32` (`E8`) in `hays`: (address, target).
pub(crate) fn calls<'a>(hays: &'a [Hay<'a>]) -> impl Iterator<Item = (usize, usize)> + 'a {
    hays.iter().flat_map(|&(start, code)| {
        memchr::memchr_iter(0xE8, code).filter_map(move |i| Some((start + i, rel32_target(start, code, i, 1)?)))
    })
}

/// Every direct branch with a rel32 operand (`call` `E8`, `jmp` `E9`, `j<cc>` `0F 8x`) in `hays`: (address, target,
/// whether it is a `call`).
pub(crate) fn branches<'a>(hays: &'a [Hay<'a>]) -> impl Iterator<Item = (usize, usize, bool)> + 'a {
    hays.iter().flat_map(|&(start, code)| {
        memchr::memchr3_iter(0xE8, 0xE9, 0x0F, code).filter_map(move |i| {
            let op = match code[i] {
                0x0F if code.get(i + 1)? & 0xF0 == 0x80 => 2,
                0x0F => return None,
                _ => 1,
            };
            Some((start + i, rel32_target(start, code, i, op)?, code[i] == 0xE8))
        })
    })
}

/// Where the branch at offset `i` of `code` (at `start`) goes: an `op`-byte opcode, then a rel32.
fn rel32_target(start: usize, code: &[u8], i: usize, op: usize) -> Option<usize> {
    let rel = i32::from_le_bytes(*code.get(i + op..)?.first_chunk()?);
    Some((start + i + op + 4).wrapping_add(rel as isize as usize))
}

/// Offsets of every (possibly overlapping) occurrence of `needle`, 16 candidates at a time: positions where its two
/// rarest bytes match (SSE2), then a full compare. An empty needle occurs everywhere, end included.
fn occurrences(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let n = needle.len();
    if n == 0 {
        return (0..=hay.len()).collect();
    }
    let mut out = Vec::new();
    let Some(last) = hay.len().checked_sub(n) else { return out };
    let (i1, i2) = rare_pair(needle);
    let full = |i: usize| hay[i..i + n] == *needle;
    let mut i = 0;
    // SAFETY: each load reads 16 bytes from `i + k` (k < n) with `i + 15 <= last`, so inside `hay`; SSE2 is the
    // baseline.
    unsafe {
        let (v1, v2) = (_mm_set1_epi8(needle[i1] as i8), _mm_set1_epi8(needle[i2] as i8));
        let at = |k: usize| _mm_loadu_si128(hay.as_ptr().add(k).cast::<__m128i>());
        while i + 15 <= last {
            let eq = _mm_and_si128(_mm_cmpeq_epi8(at(i + i1), v1), _mm_cmpeq_epi8(at(i + i2), v2));
            let mut m = _mm_movemask_epi8(eq) as u32;
            while m != 0 {
                let j = i + m.trailing_zeros() as usize;
                if full(j) {
                    out.push(j);
                }
                m &= m - 1;
            }
            i += 16;
        }
    }
    out.extend((i..=last).filter(|&j| full(j)));
    out
}

/// Positions of `needle`'s two least common bytes in x86 code (by a rough rank of common opcode and operand bytes).
fn rare_pair(needle: &[u8]) -> (usize, usize) {
    const COMMON: &[u8] = &[
        0x00, 0xFF, 0x8B, 0x89, 0x83, 0x24, 0x45, 0x04, 0xE8, 0x08, 0xC4, 0x85, 0x0C, 0x10, 0x8D, 0x74, 0x75, 0x01,
        0xC0, 0x50, 0x44, 0x0F, 0x55, 0xEC, 0x5D, 0xC3, 0x14, 0x18, 0x56, 0x53,
    ];
    let rank = |b: u8| COMMON.iter().position(|&c| c == b).map_or(0, |p| COMMON.len() - p);
    let mut order: Vec<usize> = (0..needle.len()).collect();
    order.sort_by_key(|&i| rank(needle[i]));
    let (a, b) = (order[0], *order.get(1).unwrap_or(&order[0]));
    (a.min(b), a.max(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_decode_their_rel32_targets() {
        // call +0, jmp -10, jne +0x100, a 0F that is no branch, then a call cut off at the end.
        let code = [0xE8, 0, 0, 0, 0, 0xE9, 0xF6, 0xFF, 0xFF, 0xFF, 0x0F, 0x85, 0, 1, 0, 0, 0x0F, 0xAF, 0xE8, 1, 2];
        let hays = [(0x1000, &code[..])];
        assert_eq!(calls(&hays).collect::<Vec<_>>(), [(0x1000, 0x1005)]);
        let all = [(0x1000, 0x1005, true), (0x1005, 0x1000, false), (0x100A, 0x1110, false)];
        assert_eq!(branches(&hays).collect::<Vec<_>>(), all);
        assert_eq!(find_calls(&hays, 0x1005), [0x1000]);
    }

    #[test]
    fn occurrences_match_a_plain_scan() {
        let hay: Vec<u8> = (0..300u32).map(|i| [0x8B, 0x45, 0xC3, 0x00, 0x8B][(i * 7 % 11 % 5) as usize]).collect();
        for needle in [&[0x8B][..], &[0x8B, 0x45], &[0xC3, 0x00, 0x8B], &[0x45, 0xC3, 0x00, 0x8B, 0x8B], &[0x99]] {
            for len in [0, 1, 15, 16, 17, 40, 300] {
                let h = &hay[..len];
                let want: Vec<usize> =
                    (0..(h.len() + 1).saturating_sub(needle.len())).filter(|&i| h[i..].starts_with(needle)).collect();
                assert_eq!(occurrences(h, needle), want, "needle {needle:?} len {len}");
            }
        }
    }
}
