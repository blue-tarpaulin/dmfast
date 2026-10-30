//! BYOND's core library is patched in memory once mapped.
//! Each feature is a `static` `base::feature::Feature` in `dmfast_patches` (public) or `dmfast_private`,
//! grouped by program (`compiler`, `server`) and kind (`perf`, `fixes`); [`features`] lists them in install order.
//! The loader entry hands `CONFIG` to `base::driver`, which installs the loading program's features (`base::feature::Host`).
//!
//! Features are located by signature and validated completely before writing, and
//! a mismatch is skipped with a warning.

#![allow(non_upper_case_globals)] // exported symbol names

use dmfast_base as base;

mod features;

#[cfg(test)]
mod validate;

const VERSION: &str = concat!("DMFAST ", env!("CARGO_PKG_VERSION"));

static CONFIG: base::driver::Config = base::driver::Config {
    lists: &[
        features::COMPILER_PERF,
        features::SERVER_PERF,
        features::COMPILER_FIXES,
        features::SERVER_FIXES,
        features::COMMON_FIXES,
    ],
    version: VERSION,
    after_install: dmfast_patches::compiler::perf::banner::finish_banners,
    #[cfg(windows)]
    deps: dmfast_private::deps::WINDOWS,
    #[cfg(target_os = "linux")]
    deps: dmfast_private::deps::LINUX,
};

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut std::ffi::c_void, reason: u32, _reserved: *mut std::ffi::c_void) -> i32 {
    // SAFETY: this is DllMain.
    unsafe { base::windows::dll_main(&CONFIG, reason) }
}

#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static INIT: extern "C" fn() = init;

#[cfg(target_os = "linux")]
extern "C" fn init() {
    // SAFETY: this is the library's constructor.
    unsafe { base::linux::init(&CONFIG) }
}

/// `VERSION`, NUL-terminated: lets the installer tell which dmfast version a library file is.
#[unsafe(no_mangle)]
#[used]
pub static dmfast_canary: [u8; VERSION.len() + 1] = {
    let mut out = [0; VERSION.len() + 1];
    out.split_at_mut(VERSION.len()).0.copy_from_slice(VERSION.as_bytes());
    out
};
