//! Windows: give BYOND programs an extra import, `dmfast.dll`, which patches byondcore.dll in memory and
//! pulls in `mimalloc.dll`, dmfast's allocator (mimalloc for small blocks, the process heap for large
//! ones; source in `vendor/mimalloc-x86`). dm.exe is always patched, and DreamDaemon unless `--no-runtime` is given
//! (`dreamdaemon.exe`, `dd.exe`). Each patched program's original is kept as `<name>_orig.exe`;
//! byondcore.dll and every other program stay stock.
//!
//! DreamSeeker is not supported. Install and uninstall put back what earlier builds did to it: a patched
//! dreamseeker.exe, and 4.7.0's `bin\WebView2Loader.dll` proxy (stock loader kept in `<byond>\dmfast\`).

mod elevate;
mod pe;

use std::fs;
use std::path::{Path, PathBuf};

pub use elevate::{pause_if_own_console, relaunch_elevated};

use crate::files::{Failure, io_err, read, rename_retry, write};

const DMFAST_DLL: &[u8] = include_bytes!(env!("DMFAST_LIB"));
/// What dmfast.dll needs next to it: the allocator.
const SUPPORT: [(&str, &[u8]); 1] = [("mimalloc.dll", include_bytes!("../../../vendor/mimalloc-x86/mimalloc.dll"))];
/// Files earlier builds installed next to dmfast.dll that are no longer used; install and uninstall remove them.
const OBSOLETE: [&str; 1] = ["mimalloc-redirect32.dll"];
/// Marks 4.7.0's WebView2Loader.dll proxy (an export of that name).
const PROXY_MARKER: &[u8] = b"dmfast_seekerproxy";
const LOADER: &str = "WebView2Loader.dll";
/// Sibling of `bin\` where 4.7.0 kept the stock loader, dmfast.dll and its support DLLs for the proxy.
const SIDE_DIR: &str = "dmfast";
const DLL: &str = "dmfast.dll";
const COMPILER: &str = "dm.exe";
/// Patched unless `--no-runtime` is given. Missing ones are skipped (not every BYOND build ships all of them).
const RUNTIME: [&str; 2] = ["dreamdaemon.exe", "dd.exe"];
/// Programs an earlier build may have patched that are no longer installed into; install and uninstall
/// put them back to stock if they find them patched (DreamSeeker's import rewrite crashed it).
const RETIRED: [&str; 1] = ["dreamseeker.exe"];

fn backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(format!("{}_orig.exe", exe.trim_end_matches(".exe")))
}

/// Fails if the program (or a BYOND program using byondcore.dll) is running: images cannot be opened
/// for writing.
fn ensure_not_in_use(p: &Path) -> Result<(), Failure> {
    fs::OpenOptions::new().write(true).open(p).map(drop).map_err(|e| io_err("cannot open", p, e))
}

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let dir = if target.is_file() { target.parent().unwrap_or(Path::new(".")).to_path_buf() } else { target.to_path_buf() };
    if !dir.join(COMPILER).is_file() {
        return Err(format!("no {COMPILER} in {}", dir.display()).into());
    }
    Ok(dir)
}

/// The programs to patch: the compiler, plus the runtime programs present in `dir` if asked.
fn targets(dir: &Path, runtime: bool) -> Vec<&'static str> {
    let rt = RUNTIME.into_iter().filter(|e| runtime && dir.join(e).is_file());
    std::iter::once(COMPILER).chain(rt).collect()
}

/// `<byond>\dmfast\`, where 4.7.0's proxy looked for what it loads. None if `dir` itself is named `dmfast`
/// (then nothing is removed from it).
fn side_dir(dir: &Path) -> Option<PathBuf> {
    let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case(SIDE_DIR)) {
        return None;
    }
    Some(dir.parent()?.join(SIDE_DIR))
}

/// Remove 4.7.0's DreamSeeker proxy: put the stock loader back and remove `<byond>\dmfast\`. Returns whether
/// the proxy was there.
fn remove_seeker_proxy(dir: &Path) -> Result<bool, Failure> {
    let loader = dir.join(LOADER);
    let Some(side) = side_dir(dir) else { return Ok(false) };
    let stock = side.join(LOADER);
    let proxied = loader.is_file() && pe::exports(&read(&loader)?, PROXY_MARKER);
    if proxied {
        ensure_not_in_use(&loader)?;
        if !stock.is_file() {
            return Err(format!("{} is missing; reinstall BYOND to restore {LOADER}", stock.display()).into());
        }
        rename_retry(&stock, &loader).map_err(|e| io_err("cannot restore", &loader, e))?;
    }
    for name in [DLL, LOADER].into_iter().chain(SUPPORT.map(|(n, _)| n)).chain(OBSOLETE) {
        let p = side.join(name);
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    // Only if empty: it was ours, but never delete what someone else put there.
    let _ = fs::remove_dir(&side);
    Ok(proxied)
}

/// Rewrite one program to import dmfast.dll first, keeping the stock one as its `_orig` backup.
/// Returns what was there before (`stock`, or the dmfast version it had).
fn patch(dir: &Path, name: &str, installed: &Option<String>) -> Result<String, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    let exe_data = read(&exe)?;
    let (stock, before) = if pe::imports(&exe_data, DLL)? {
        // Already patched: rebuild from the backup.
        if !backup.is_file() {
            return Err(format!("{name} has dmfast but {} is missing; reinstall BYOND", backup.display()).into());
        }
        (read(&backup)?, installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into()))
    } else {
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
        write(&backup, &exe_data)?;
        (exe_data, before.to_string())
    };
    write(&exe, &pe::add_imports_first(&stock, &[(DLL, "dmfast_mimalloc_version")])?)?;
    Ok(before)
}

/// Put the original back if `name` is patched. Returns whether it was.
fn restore(dir: &Path, name: &str) -> Result<bool, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if !exe.is_file() || !pe::imports(&read(&exe)?, DLL)? {
        return Ok(false);
    }
    if !backup.is_file() {
        return Err(format!("{} is missing; reinstall BYOND to restore {name}", backup.display()).into());
    }
    rename_retry(&backup, &exe).map_err(|e| io_err("cannot restore", &exe, e))?;
    Ok(true)
}

pub fn install(dir: &Path, runtime: bool) -> Result<String, Failure> {
    let names = targets(dir, runtime);
    for name in &names {
        ensure_not_in_use(&dir.join(name))?;
    }
    let core = dir.join("byondcore.dll");
    let compiler_data = read(&dir.join(COMPILER))?;
    if pe::has_section(&compiler_data, b".dmimp") || (core.is_file() && pe::has_section(&read(&core)?, b".dmfast")) {
        return Err("this BYOND has dmfast's old on-disk patches; \
                    put byondcore.dll.orig and dm.exe.orig back first"
            .into());
    }
    let ours = pe::canary(DMFAST_DLL).ok_or("internal: embedded dmfast.dll has no canary")?;
    let dll = dir.join(DLL);
    let installed = dll.is_file().then(|| read(&dll)).transpose()?.and_then(|d| pe::canary(&d));
    let mut before = String::new();
    for name in &names {
        let b = patch(dir, name, &installed)?;
        if *name == COMPILER {
            before = b;
        }
    }
    // With --no-runtime, a previous runtime install is undone (re-running with it means
    // "compiler only").
    let mut unpatched = Vec::new();
    for name in RUNTIME.into_iter().chain(RETIRED).filter(|n| !names.contains(n)) {
        if restore(dir, name)? {
            unpatched.push(name);
        }
    }
    write(&dll, DMFAST_DLL)?;
    for (name, data) in SUPPORT {
        write(&dir.join(name), data)?;
    }
    for name in OBSOLETE {
        let p = dir.join(name);
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    if remove_seeker_proxy(dir)? {
        unpatched.push("dreamseeker.exe (WebView2Loader.dll)");
    }
    let programs = names.join(", ");
    let mut msg = format!("installed {ours} in {} (was {before}) for {programs}", dir.display());
    if !unpatched.is_empty() {
        msg += &format!("; restored stock {}", unpatched.join(", "));
    }
    Ok(msg)
}

pub fn uninstall(dir: &Path) -> Result<String, Failure> {
    let dll = dir.join(DLL);
    let all = std::iter::once(COMPILER).chain(RUNTIME).chain(RETIRED);
    for name in all.clone() {
        let p = dir.join(name);
        if p.is_file() {
            ensure_not_in_use(&p)?;
        }
    }
    let ver = dll.is_file().then(|| read(&dll)).transpose()?.and_then(|d| pe::canary(&d));
    let mut restored = Vec::new();
    for name in all {
        if restore(dir, name)? {
            restored.push(name);
        }
    }
    if remove_seeker_proxy(dir)? {
        restored.push(LOADER);
    }
    if restored.is_empty() {
        return Ok(format!("dmfast is not installed in {}", dir.display()));
    }
    for p in [dll].into_iter().chain(SUPPORT.map(|(name, _)| dir.join(name))).chain(OBSOLETE.map(|name| dir.join(name))) {
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    Ok(format!(
        "removed {} from {} (restored {})",
        ver.unwrap_or_else(|| "dmfast".into()),
        dir.display(),
        restored.join(", ")
    ))
}
