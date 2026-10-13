//! Windows: give BYOND programs an extra import, `dmfast.dll`, which patches byondcore.dll in memory and
//! pulls in `mimalloc.dll`, dmfast's allocator (mimalloc for small blocks, the process heap for large
//! ones; source in `vendor/mimalloc-x86`). dm.exe is always patched; with `--runtime` also DreamDaemon
//! (`dreamdaemon.exe`, `dd.exe`). Each patched program's original is kept as `<name>_orig.exe`;
//! byondcore.dll and every other program stay stock.
//!
//! DreamSeeker is not rewritten: it refuses to run properly when an unknown DLL from `bin\` is loaded. The
//! planned route replaces `bin\WebView2Loader.dll` (which DreamSeeker's byondwin.dll imports first; BYOND 515
//! has none) with a proxy ([`PROXY`]) that forwards to the stock loader and loads dmfast.dll, both kept in
//! `<byond>\dmfast\` outside `bin\`. It is still in development and off ([`SEEKER_ENABLED`]): every
//! install removes a proxy an earlier build put there.

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
/// Stand-in for WebView2Loader.dll that loads dmfast into DreamSeeker (marked by the [`PROXY_MARKER`] export).
const PROXY: &[u8] = include_bytes!(env!("SEEKERPROXY_LIB"));
const PROXY_MARKER: &[u8] = b"dmfast_seekerproxy";
const LOADER: &str = "WebView2Loader.dll";
/// Sibling of `bin\` holding what DreamSeeker's proxy loads: the stock loader, dmfast.dll and its support DLLs.
const SIDE_DIR: &str = "dmfast";
const SEEKER: &str = "dreamseeker.exe";
const DLL: &str = "dmfast.dll";
const COMPILER: &str = "dm.exe";
/// Patched with `--runtime`. Missing ones are skipped (not every BYOND build ships all of them).
///
/// DreamSeeker is not in this list: any rewrite of its import table, or any unknown DLL in `bin\`, trips
/// its loaded-module scan and it crashes later in `DMString::operator=`. See [`install_seeker`].
const RUNTIME: [&str; 2] = ["dreamdaemon.exe", "dd.exe"];
/// Programs an earlier build may have patched that are no longer installed into; install and uninstall
/// put them back to stock if they find them patched (DreamSeeker's import rewrite crashed it).
const RETIRED: [&str; 1] = [SEEKER];
/// DreamSeeker support through the WebView2Loader.dll proxy: in development, not installed yet.
const SEEKER_ENABLED: bool = false;

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

/// `<byond>\dmfast\`, where the proxy looks for what it loads. None if `dir` itself is named `dmfast`:
/// the side folder would be `bin\` itself, so DreamSeeker is left alone (and nothing is removed from it).
fn side_dir(dir: &Path) -> Option<PathBuf> {
    let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case(SIDE_DIR)) {
        return None;
    }
    Some(dir.parent()?.join(SIDE_DIR))
}

fn is_proxy(p: &Path) -> Result<bool, Failure> {
    Ok(p.is_file() && pe::exports(&read(p)?, PROXY_MARKER))
}

/// Load dmfast into DreamSeeker: stock loader, dmfast.dll and its support DLLs go to `<byond>\dmfast\`, the proxy
/// takes the loader's place in `bin\`. Returns false (nothing done) if this BYOND has no DreamSeeker.
fn install_seeker(dir: &Path) -> Result<bool, Failure> {
    let loader = dir.join(LOADER);
    if !dir.join(SEEKER).is_file() || !loader.is_file() {
        return Ok(false);
    }
    let Some(side) = side_dir(dir) else { return Ok(false) };
    ensure_not_in_use(&loader)?;
    let stock = side.join(LOADER);
    let proxied = is_proxy(&loader)?;
    if proxied && !stock.is_file() {
        return Err(format!("{} is missing; reinstall BYOND to restore {LOADER}", stock.display()).into());
    }
    fs::create_dir_all(&side).map_err(|e| io_err("cannot create", &side, e))?;
    write(&side.join(DLL), DMFAST_DLL)?;
    for (name, data) in SUPPORT {
        write(&side.join(name), data)?;
    }
    if !proxied {
        // Whatever is in bin\ is BYOND's own (possibly newer than an older copy kept here).
        rename_retry(&loader, &stock).map_err(|e| io_err("cannot move", &loader, e))?;
    }
    write(&loader, PROXY)?;
    Ok(true)
}

/// Put the stock loader back and remove `<byond>\dmfast\`. Returns whether the proxy was installed.
fn uninstall_seeker(dir: &Path) -> Result<bool, Failure> {
    let loader = dir.join(LOADER);
    let Some(side) = side_dir(dir) else { return Ok(false) };
    let stock = side.join(LOADER);
    let proxied = is_proxy(&loader)?;
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
    // Only if empty: it is ours, but never delete what someone else put there.
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
    write(&exe, &pe::add_import_first(&stock, DLL, "dmfast_mimalloc_version")?)?;
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
    // Without --runtime, a previous runtime install is undone (re-running the plain install means
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
    let mut programs = names.join(", ");
    if runtime && SEEKER_ENABLED && install_seeker(dir)? {
        programs += ", dreamseeker.exe (via WebView2Loader.dll)";
    } else if uninstall_seeker(dir)? {
        unpatched.push("dreamseeker.exe (WebView2Loader.dll)");
    }
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
    if uninstall_seeker(dir)? {
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
