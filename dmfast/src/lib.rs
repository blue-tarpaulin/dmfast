//! BYOND's core library is patched in memory once mapped.
//! Each feature is a `static` `base::feature::Feature` in `dmfast_patches`, which lists them in install order
//! (`FEATURES`); the private submodule's follow. The dependency guards are in `dmfast_patches::deps`.
//! The loader entry hands `CONFIG` to `base::driver`, which installs the loading program's features (`base::feature::Host`).
//!
//! Features are located by signature and validated completely before writing, and
//! a mismatch is skipped with a warning.

#![allow(non_upper_case_globals)] // exported symbol names

use dmfast_base as base;

/// The private submodule's patches, built in when it is checked out (`build.rs`).
#[cfg(dmfast_private)]
#[path = "../../patches-private/src/lib.rs"]
mod private;
#[cfg(dmfast_private)]
const PRIVATE: &[&base::feature::Feature] = private::FEATURES;
#[cfg(not(dmfast_private))]
const PRIVATE: &[&base::feature::Feature] = &[];

#[cfg(test)]
mod validate;

const VERSION: &str = concat!("DMFAST ", env!("CARGO_PKG_VERSION"));

static CONFIG: base::driver::Config = base::driver::Config {
    lists: &[dmfast_patches::FEATURES, PRIVATE],
    version: VERSION,
    #[cfg(windows)]
    deps: &[dmfast_patches::deps::WINDOWS],
    #[cfg(target_os = "linux")]
    deps: &[dmfast_patches::deps::LINUX],
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
