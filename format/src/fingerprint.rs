//! The hash a function's fingerprint keeps instead of its bytes (dmfast's dependency guards, and a DreamSeeker
//! patch's `site fingerprint`).

/// FNV-1a 64 over `bytes`, skipping the (offset, length) spans in `holes` (in order).
pub fn fnv1a(bytes: &[u8], holes: &[(u16, u16)]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |s: &[u8]| {
        for &b in s {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut at = 0;
    for &(off, n) in holes {
        let off = (off as usize).clamp(at, bytes.len());
        eat(&bytes[at..off]);
        at = (off + n as usize).min(bytes.len());
    }
    eat(&bytes[at..]);
    h
}
