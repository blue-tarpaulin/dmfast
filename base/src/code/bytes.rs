//! Bytes for code rewritten in place ([`branch`], [`pad`]) and for patterns ([`hx`]).

use super::CodeAddr;

/// `E8`/`E9` + rel32 from `at` to `target`, padded with NOPs to `len`: bytes for a larger in-place rewrite
/// ([`crate::image::Image::write_code`]); for a lone call or jump use `Image::call` or `Image::jump`. Panics (which
/// aborts) if `len < 5`; `Image::call` and `Image::jump` return `Err` instead.
pub fn branch(op: u8, at: usize, target: impl CodeAddr, len: usize) -> Vec<u8> {
    assert!(len >= 5, "branch: len {len} < 5");
    let mut v = vec![op];
    v.extend_from_slice(&(target.code_addr().wrapping_sub(at + 5) as u32).to_le_bytes());
    v.resize(len, 0x90);
    v
}

/// `code` followed by NOPs up to `N` bytes, at compile time: new code for an in-place rewrite of an `N`-byte range.
pub const fn pad<const N: usize>(code: &[u8]) -> [u8; N] {
    let mut out = [0x90; N];
    out.split_at_mut(code.len()).0.copy_from_slice(code);
    out
}

/// IDA-style bytes of a little-endian u32 (an absolute address as it appears in code), to put into a pattern.
pub fn hx(v: usize) -> String {
    (v as u32).to_le_bytes().map(|b| format!("{b:02X}")).join(" ")
}
