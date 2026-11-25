//! `offline`: DreamDaemon's `-offline` skips BYOND's own hub connection. DreamDaemon looks the hub up on its main
//! thread before the world starts, and again later, which stalls startup on a slow or missing network.
//!
//! The socket library's address lookup (`SocketLib::GetSocketAddress`) fails at once for the hub's names, as it does
//! for a name that doesn't resolve, so the hub connection gives up without a lookup or a connection. Other lookups
//! (`world.Export()` and the like) are untouched.
//!
//! The flag leaves the command line only once the hook is in; if it can't be, DreamDaemon refuses the unknown option
//! instead of starting online.

use std::ffi::{CStr, c_char, c_void};

use crate::base::args;
use crate::base::driver::Counts;
use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static OFFLINE: Feature = Feature::fix(
    Hosts::SERVER,
    "offline",
    "-offline: no BYOND hub connection",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
)
.default_on()
.offered_if(|_| args::has(FLAG))
.finish(take_flag);

const FLAG: &str = "-offline";

/// The lookup's callback: the address, or null if the name didn't resolve.
type Found = unsafe extern "C" fn(*mut c_void, *mut c_void);

fn take_flag(_: Counts) {
    // SAFETY: `finish` runs at install time, before DreamDaemon reads its command line.
    unsafe { args::remove(FLAG) }
}

/// Whether `host` (null or a C string) is the BYOND hub's: `byond.com` or a name under it.
///
/// # Safety
/// `host` is null or a C string.
unsafe fn hub(host: *const c_char) -> bool {
    if host.is_null() {
        return false;
    }
    // SAFETY: a C string (caller).
    let host = unsafe { CStr::from_ptr(host) }.to_bytes();
    let under = host.len().checked_sub(b".byond.com".len()).map(|i| &host[i..]);
    host.eq_ignore_ascii_case(b"byond.com") || under.is_some_and(|s| s.eq_ignore_ascii_case(b".byond.com"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_byond_names_are_the_hub() {
        // SAFETY: C strings.
        unsafe {
            assert!(hub(c"hub.byond.com".as_ptr()) && hub(c"BYOND.com".as_ptr()));
            assert!(!hub(c"notbyond.com".as_ptr()) && !hub(c"127.0.0.1".as_ptr()) && !hub(std::ptr::null()));
        }
    }
}
