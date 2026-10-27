//! Signature and byte searches over byte ranges of a mapped image ([`crate::image::Image`] runs them and decides
//! what is relied on). Matches may overlap, so a count includes every occurrence.

use memchr::memmem::Finder;

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
    let finder = Finder::new(&anchor);
    let mut hits = Vec::new();
    for &(start, hay) in hays {
        for at in overlapping(&finder, hay) {
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
    let mut bitmap = [0u64; 1024];
    let mut buckets: rustc_hash::FxHashMap<u16, Vec<(usize, usize, u32)>> = Default::default();
    for (j, pat) in pats.iter().enumerate() {
        let best = longest_fixed_run(pat);
        let anchor: Vec<u8> = pat[best.0..best.0 + best.1.min(4)].iter().flatten().copied().collect();
        let Ok(a) = <[u8; 4]>::try_from(anchor) else {
            hits[j] = find_pattern(hays, pat);
            continue;
        };
        let key = u16::from_le_bytes([a[0], a[1]]);
        bitmap[key as usize / 64] |= 1 << (key % 64);
        buckets.entry(key).or_default().push((j, best.0, u32::from_le_bytes(a)));
    }
    for &(start, hay) in hays {
        let len = hay.len();
        for i in 0..len.saturating_sub(3) {
            let word = u32::from_le_bytes([hay[i], hay[i + 1], hay[i + 2], hay[i + 3]]);
            let key = word as u16;
            if bitmap[(key >> 6) as usize] >> (key & 63) & 1 == 0 {
                continue;
            }
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
    let finder = Finder::new(needle);
    let mut hits = Vec::new();
    for &(start, hay) in hays {
        for i in overlapping(&finder, hay) {
            hits.push(start + i);
        }
    }
    hits
}

/// Addresses of every `call rel32` (`E8`) to `target` in `hays`.
pub(crate) fn find_calls(hays: &[Hay], target: usize) -> Vec<usize> {
    let mut hits = Vec::new();
    for &(start, code) in hays {
        for i in memchr::memchr_iter(0xE8, &code[..code.len().saturating_sub(4)]) {
            let rel = i32::from_le_bytes([code[i + 1], code[i + 2], code[i + 3], code[i + 4]]);
            if (start + i + 5).wrapping_add(rel as isize as usize) == target {
                hits.push(start + i);
            }
        }
    }
    hits
}

/// Offsets of every (possibly overlapping) occurrence.
fn overlapping<'a>(finder: &'a Finder<'_>, hay: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    let mut from = 0;
    std::iter::from_fn(move || {
        let at = from + finder.find(hay.get(from..)?)?;
        from = at + 1;
        Some(at)
    })
}
