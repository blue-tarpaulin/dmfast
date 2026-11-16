//! Linux: `libdmfast.so` is loaded via `LD_PRELOAD` (the installer wraps each program in a script running the
//! stock one in `bin/backup/`) and patches libbyond.so from its constructor ([`init`]), before any thread runs it.

pub mod alloc;
pub mod glibc;
mod image;
mod patch;

pub(crate) use patch::{alloc_exec, free_exec, poke_code, poke_data, seal_exec};

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use crate::driver::Config;
use crate::feature::Host;
use crate::{Result, bail};

pub(crate) const CORE_NAME: &str = "libbyond.so";
/// The page size mappings use.
pub(crate) const PAGE: usize = 4096;

/// The library's constructor body: drop this library from `LD_PRELOAD`, install `config`'s features, and print their
/// counters at exit under `DMFAST_STATS`.
///
/// # Safety
/// Called from the constructor: every object is mapped and relocated, no other thread exists.
pub unsafe fn init(config: &'static Config) {
    remove_from_preload();
    let Some(host) = Host::current() else { return };
    // SAFETY: called from the constructor (caller).
    unsafe { crate::driver::install(config, host) };
    if crate::driver::stats() {
        // SAFETY: registers a plain `extern "C" fn()` that captures nothing.
        unsafe { libc::atexit(print_stats_at_exit) };
    }
}

extern "C" fn print_stats_at_exit() {
    crate::driver::print_stats();
}

/// What the program's calls to `name` resolve to (`dlsym(RTLD_DEFAULT)`). One object's own symbol:
/// [`Image::export`](crate::image::Image::export).
pub fn symbol(name: &std::ffi::CStr) -> Result<usize> {
    // SAFETY: a lookup by NUL-terminated name; nothing is called.
    let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) } as usize;
    if f == 0 {
        bail!("no {}", name.to_string_lossy());
    }
    Ok(f)
}

/// Keep children from inheriting this 32-bit library (64-bit ones would print a loader error).
fn remove_from_preload() {
    let Some(v) = std::env::var_os("LD_PRELOAD") else { return };
    let v = v.as_bytes();
    let rest: Vec<&[u8]> = v
        .split(|&b| b == b' ' || b == b':')
        .filter(|e| !e.is_empty() && !e.ends_with(b"/libdmfast.so") && *e != b"libdmfast.so")
        .collect();
    // SAFETY: constructors run before any other thread exists.
    unsafe {
        if rest.is_empty() {
            std::env::remove_var("LD_PRELOAD");
        } else {
            std::env::set_var("LD_PRELOAD", OsString::from_vec(rest.join(&b' ')));
        }
    }
}
