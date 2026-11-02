//! `hubsock`: a player's connection can no longer take a pending hub connection's socket.
//!
//! `SocketLib::RegisterSocketCallback` keeps one [`Entry`] per socket number and overwrites an existing one. A
//! connection (`clisock`) re-registers its socket after every send, even when that number now belongs to a pending
//! outgoing connect (mode 8, callback "connect done"). The hub connection then misreads the player's data and drops,
//! and its connect timer closes the number it remembers, by then another client's. The fix:
//! 1. Register leaves a pending connect's entry alone unless the new callback is the connect one, both when it
//!    overwrites and on the mode-0 unregister. Only that callback or `CloseSocket` removes it.
//! 2. The connection's close function closes its connecting socket only while the pending connect on that number
//!    is its own; otherwise it only forgets the number.
//!
//! Every site is checked inside its own function, so a differing build turns the fix off instead of half-applying.

use crate::base::code::live_u32;
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static HUBSOCK: Feature = Feature::fix(
    Hosts::SERVER,
    "hubsock",
    "a player link can't take a pending hub connection's socket",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// The socket library's entry for a socket number.
#[repr(C)]
struct Entry {
    socket: i32,
    callback: u32,
    name: u32,
    /// A pending connect's: its {callback, connection} pair.
    data: u32,
    mode: u8,
}

/// What install found: the "connect done" callback, the socket library, and its entry table's fields (the entry
/// pointers and their count).
struct Found {
    connect_done: u32,
    socklib: usize,
    entries: usize,
    count: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// The entry for `socket` in socket library `lib`'s table, if it is a pending connect's (the first entry for the
/// number decides, as in Register's search).
///
/// # Safety
/// `lib` is a socket library.
unsafe fn pending<'a>(f: &Found, lib: usize, socket: i32) -> Option<&'a Entry> {
    // SAFETY: the library's table (caller): `count` entry pointers, null or live.
    unsafe {
        let table = live_u32(lib + f.entries) as *const *const Entry;
        let count = live_u32(lib + f.count) as i32;
        for i in 0..count.max(0) as usize {
            if let Some(e) = (*table.add(i)).as_ref()
                && e.socket == socket
            {
                return (e.callback == f.connect_done).then_some(e);
            }
        }
        None
    }
}

/// Register's unregister of `socket` (mode 0): whether to keep the entry, a pending connect's.
///
/// # Safety
/// From the stub: `lib` is the socket library Register runs on.
unsafe extern "C" fn keeps_pending(lib: usize, socket: i32) -> bool {
    // SAFETY: the socket library (caller).
    unsafe { pending(FOUND.get(), lib, socket).is_some() }
}

/// Connection `conn`'s close of its connecting `socket`: whether the pending connect on that number is its own, so
/// closing it is right.
///
/// # Safety
/// From the stub: `conn` is the connection closing.
unsafe extern "C" fn owns_pending(socket: i32, conn: usize) -> bool {
    let f = FOUND.get();
    // SAFETY: the socket library, found at install; a pending connect's data is its {callback, connection} pair.
    unsafe { pending(f, f.socklib, socket).is_some_and(|e| live_u32(e.data as usize + 4) as usize == conn) }
}
