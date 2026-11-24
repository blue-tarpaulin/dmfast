//! `offline`, Windows: `SocketLib::GetSocketAddress` hooked at its entry.

use std::ffi::{c_char, c_void};

use super::{Found, hub};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

type Lookup = unsafe extern "thiscall" fn(*mut c_void, *const c_char, *const c_char, Found, *mut c_void) -> i32;

static ORIGINAL: Foreign<Lookup> = Foreign::new();

/// Its entry: `push ebp ; mov ebp,esp ; sub esp,24h`.
const ENTRY: &str = "55 8B EC 83 EC ??";
const ENTRY_LEN: usize = 6;

/// # Safety
/// As `SocketLib::GetSocketAddress`.
unsafe extern "thiscall" fn lookup(
    lib: *mut c_void,
    host: *const c_char,
    service: *const c_char,
    found: Found,
    data: *mut c_void,
) -> i32 {
    // SAFETY: the caller's arguments; a failed lookup calls back with null and returns -1, as the original's does.
    unsafe {
        if !hub(host) {
            return ORIGINAL.get()(lib, host, service, found, data);
        }
        found(std::ptr::null_mut(), data);
    }
    -1
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.export(c"?GetSocketAddress@SocketLib@@QAEJPBD0P6AXPAUsockaddr_in@@PAX@Z2@Z")?;
    core.require(f, ENTRY, "GetSocketAddress entry")?;
    ORIGINAL.set(core.hook(f, ENTRY_LEN, lookup as Lookup)?);
    Ok(core.report(&[f]))
}
