//! Windows: the installer adds a `dmfast.dll` import to dm.exe (original kept as dm_orig.exe) and, unless
//! `--no-runtime`, to dreamdaemon.exe and dd.exe. byondcore.dll is patched in memory from [`dll_main`] with the
//! loading program's features ([`crate::feature::Host`], [`crate::driver`]).

pub(crate) mod alloc;
pub mod mem;
pub mod ucrt;

use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};

use crate::driver::Config;
use crate::feature::Host;

pub(crate) const CORE_NAME: &str = "byondcore.dll";

/// `DllMain` body: on attach install `config`'s features of the host, on detach print counters (`DMFAST_STATS`).
/// # Safety
/// Called from `DllMain` with its `reason`: all static imports are mapped and relocated, no other thread runs.
pub unsafe fn dll_main(config: &'static Config, reason: u32) -> i32 {
    match (reason, Host::current()) {
        (DLL_PROCESS_ATTACH, Some(host)) => unsafe { crate::driver::install(config, host) },
        (DLL_PROCESS_DETACH, Some(_)) => crate::driver::print_stats(),
        _ => {}
    }
    1
}
