//! `utf8leak`: making a string from text that isn't UTF-8 (`html_decode("&eacute;")`, `url_decode("%E9")`) no
//! longer leaks memory.
//!
//! BYOND's string creation converts such text to a fresh UTF-8 copy, then marks it as the caller's, so the copy is
//! duplicated again for the string table (or kept when the string already exists) and never freed. The fix marks the
//! converted copy as the function's own, as it is: the table takes it, or the function frees it.

use crate::base::Result;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};

pub static UTF8LEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "utf8leak",
    "a string converted to UTF-8 is owned by the string table",
    Tier::Safe,
    install!(windows: install, linux: install),
);

/// After the conversion: (keep the copy) ; encoding = UTF-8 ; caller owns the text = 1.
#[cfg(windows)]
const OWNED: &str = "89 75 E0 C6 45 FC 02 C6 45 14 01";
#[cfg(target_os = "linux")]
const OWNED: &str = "89 5C 24 20 C6 44 24 3C 02 C6 44 24 0C 01 E9";
/// The flag's immediate.
#[cfg(windows)]
const FLAG: usize = 10;
#[cfg(target_os = "linux")]
const FLAG: usize = 13;

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(OWNED, "string creation: converted text")?;
    core.write_code(at + FLAG, &[0])?;
    Ok(core.report(&[at]))
}
