//! `stdio` and `ftell`: the compiler's character reads and .dmb writes without the stream lock, and the .dmb string
//! writer's `ftell` without a syscall.
//!
//! Only the compiler thread reads source files and writes the .dmb: Windows checks this in verify mode, and Linux
//! leaves stdin, stdout, and stderr (the progress thread's too) to the locked calls. Each platform's file says why
//! its `ftell` answer is exact; `DMFAST_VERIFY=1` also asks the CRT and counts disagreements.
//!
//! Dependency guards (`deps`): `stdio` none, since it only redirects CRT calls at whole-matched sites; Windows
//! `ftell` the string table save and the string writer; Linux `ftell` none, since it reads glibc's own stream state.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;
mod verify;
#[cfg(windows)]
mod windows;

pub static STDIO: Feature =
    Feature::perf(Hosts::COMPILER, "stdio", "stdio", install!(windows: windows::install, linux: linux::install))
        .stats(stdio_stats);

pub static FTELL: Feature = Feature::perf(
    Hosts::COMPILER,
    "ftell",
    "ftell cache",
    install!(windows: windows::install_ftell, linux: linux::install_ftell),
)
.after(&[&STDIO])
.stats(ftell_stats);

fn stdio_stats() {
    #[cfg(windows)]
    windows::stdio_stats();
}

fn ftell_stats() {
    #[cfg(windows)]
    windows::ftell_stats();
    #[cfg(target_os = "linux")]
    linux::ftell_stats();
}
