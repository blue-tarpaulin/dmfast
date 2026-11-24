//! Linux: `argv` as the constructor got it. `main`'s `argc` is already fixed, so the arguments after a removed flag
//! move down and the last becomes `""`, which DreamDaemon skips.

use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

/// `argc` and the `argv` array's address.
static ARGV: OnceLock<(usize, usize)> = OnceLock::new();

/// Keep the constructor's `argc` and `argv`.
pub(crate) fn set_argv(argc: usize, argv: *mut *const c_char) {
    let _ = ARGV.set((argc, argv as usize));
}

pub(super) unsafe fn remove(flag: &str) {
    let Some(&(argc, argv)) = ARGV.get().filter(|&&(argc, argv)| argc > 1 && argv != 0) else { return };
    // SAFETY: the process's `argv` entries after the name, C strings; install time, so no one else uses them (caller).
    let args = unsafe { std::slice::from_raw_parts_mut((argv as *mut *const c_char).add(1), argc - 1) };
    let mut kept = 0;
    for i in 0..args.len() {
        // SAFETY: an `argv` entry.
        if !unsafe { CStr::from_ptr(args[i]) }.to_bytes().eq_ignore_ascii_case(flag.as_bytes()) {
            args[kept] = args[i];
            kept += 1;
        }
    }
    args[kept..].fill(c"".as_ptr());
}
