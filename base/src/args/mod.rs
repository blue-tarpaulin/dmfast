//! Flags dmfast takes for itself: a feature checks for its flag at install and removes it once installed, so the
//! program never sees it (and refuses it as an unknown option if the feature didn't install). A flag is a whole
//! argument after the program's name, any case.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "linux")]
pub(crate) use linux::set_argv;
#[cfg(windows)]
use windows as os;

/// Whether the command line has `flag`.
pub fn has(flag: &str) -> bool {
    std::env::args_os().skip(1).any(|a| a.eq_ignore_ascii_case(flag))
}

/// Take every `flag` off the command line.
///
/// # Safety
/// Install time: the program hasn't read its command line yet, and no other thread runs.
pub unsafe fn remove(flag: &str) {
    // SAFETY: install time (caller).
    unsafe { os::remove(flag) }
}
