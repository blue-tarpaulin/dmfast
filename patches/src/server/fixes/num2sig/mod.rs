//! `num2sig` (forum report 2960272): `num2text(10, 4)` gives `10.00`, not `10`. The digits go to
//! `sprintf("%.*g")`, which drops trailing zeros; the two-argument form's call goes to [`sig_g`] instead, which keeps
//! them. `num2text(x)` and `"[x]"` keep their output.

use crate::base::code::Foreign;
use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static NUM2SIG: Feature = Feature::fix(
    Hosts::SERVER,
    "num2sig",
    "num2text keeps trailing zeros",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

type Sprintf = unsafe extern "C" fn(*mut u8, *const u8, ...) -> i32;

/// The `sprintf` BYOND calls.
static SPRINTF: Foreign<Sprintf> = Foreign::new();

/// `sprintf(buf, "%.*g", digits, value)`, but with the trailing zeros `%g` drops.
///
/// # Safety
/// `buf` is BYOND's buffer for this call, as large as `%#.*g` needs.
unsafe extern "C" fn sig_g(buf: *mut u8, _fmt: *const u8, digits: i32, value: f64) -> i32 {
    // SAFETY: SPRINTF is the sprintf BYOND called here; `buf` holds its output and the NUL (caller).
    let s = unsafe {
        let len = SPRINTF.get()(buf, c"%#.*g".as_ptr().cast(), digits, value) as usize;
        std::slice::from_raw_parts_mut(buf, len + 1)
    };
    let len = s.len() - 1;
    // "100." -> "100", "1.e+10" -> "1e+10".
    if let Some(dot) = s[..len].iter().position(|&c| c == b'.')
        && matches!(s[dot + 1], 0 | b'e')
    {
        s.copy_within(dot + 1.., dot);
        return len as i32 - 1;
    }
    len as i32
}
