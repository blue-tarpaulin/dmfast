//! Windows: the installer adds a `dmfast.dll` import to dm.exe (original kept as dm_orig.exe) unless `--no-compiler`,
//! and to dreamdaemon.exe and dd.exe unless `--no-server`. byondcore.dll is patched in memory from [`dll_main`] with the
//! loading program's features ([`crate::feature::Host`], [`crate::driver`]).

pub(crate) mod alloc;
pub(crate) mod mem;
mod seeker;
pub mod ucrt;

pub use mem::module_range;

use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};

use crate::driver::Config;
use crate::feature::Host;

pub(crate) const CORE_NAME: &str = "byondcore.dll";

/// `DllMain` body: on attach put BYOND's code back at DreamSeeker's patch sites, then install `config`'s features of
/// the host; on detach print counters (`DMFAST_STATS`).
/// # Safety
/// Called from `DllMain` with its `reason`: all static imports are mapped and relocated, no other thread runs.
pub unsafe fn dll_main(config: &'static Config, reason: u32) -> i32 {
    match (reason, Host::current()) {
        // SAFETY: loader entry (caller): the DLLs are mapped and relocated, and no other thread runs.
        (DLL_PROCESS_ATTACH, Some(host)) => unsafe {
            seeker::restore_sites();
            crate::driver::install(config, host)
        },
        (DLL_PROCESS_DETACH, Some(_)) => crate::driver::print_stats(),
        _ => {}
    }
    1
}
