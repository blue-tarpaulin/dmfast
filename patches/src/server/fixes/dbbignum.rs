//! `dbbignum` (forum report 2988144): database numbers between 8.32 billion and 2^33 (and their negatives) are no
//! longer written as NULL.
//!
//! The SQL-literal writer tests a float for infinity or NaN with the mask `0x07F80000` instead of `0x7F800000`
//! (`and eax,mask ; cmp eax,mask`), so values like `[0x1F0000000, 0x200000000)`, and the same range in each binade,
//! become `NULL` or `1e2000`. The fix writes the right mask in both instructions with one write.

use crate::base::Result;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};

pub static DBBIGNUM: Feature = Feature::fix(
    Hosts::SERVER,
    "dbbignum",
    "database numbers 8.32e9 to 2^33 are not NULL",
    Tier::Safe,
    install!(windows: install, linux: install),
);

/// `mov eax,esi ; and eax,07F80000h ; cmp eax,07F80000h ; jne ..`, then the NaN test of the low bits.
#[cfg(windows)]
const TEST: &str = "8B C6 25 00 00 F8 07 3D 00 00 F8 07 75 ?? F7 C6 FF FF 07 00";
#[cfg(target_os = "linux")]
const TEST: &str = "89 F0 25 00 00 F8 07 3D 00 00 F8 07 75 ?? DD D8 F7 C6 FF FF 07 00";

/// The two masks from the `and`'s immediate: `mask ; cmp eax, ; mask` (the `cmp` opcode unchanged).
const MASKS: [u8; 9] = {
    let [a, b, c, d] = 0x7F80_0000u32.to_le_bytes();
    [a, b, c, d, 0x3D, a, b, c, d]
};

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(TEST, "SQL number literal")?;
    core.write_code(at + 3, &MASKS)?;
    Ok(format!("at +{:#x}", at - core.base))
}
