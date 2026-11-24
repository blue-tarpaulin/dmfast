//! `offline`, Linux: `SocketLib::GetSocketAddress` hooked at its entry.

use std::ffi::{c_char, c_long, c_void};

use super::{Found, hub};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;

type Lookup = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, Found, *mut c_void) -> c_long;

static ORIGINAL: Foreign<Lookup> = Foreign::new();

/// Its entry: `push esi ; push ebx ; sub esp,4`.
const ENTRY: &str = "56 53 83 EC 04";
const ENTRY_LEN: usize = 5;

/// # Safety
/// As `SocketLib::GetSocketAddress`.
unsafe extern "C" fn lookup(
    lib: *mut c_void,
    host: *const c_char,
    service: *const c_char,
    found: Found,
    data: *mut c_void,
) -> c_long {
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
    let f = core.export(c"_ZN9SocketLib16GetSocketAddressEPKcS1_PFvP11sockaddr_inPvES4_")?;
    core.require(f, ENTRY, "GetSocketAddress entry")?;
    ORIGINAL.set(core.hook(f, ENTRY_LEN, lookup as Lookup)?);
    Ok(core.report(&[f]))
}
