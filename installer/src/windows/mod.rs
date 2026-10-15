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

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

pub use elevate::{own_console, pause_if_own_console, relaunch_elevated};

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

/// For the guided install: whether dmfast is installed in `dir`, and whether DreamDaemon is patched.
pub fn state(dir: &Path) -> (bool, bool) {
    (dir.join(DLL).is_file(), RUNTIME.iter().any(|exe| backup_of(dir, exe).is_file()))
}

/// Where the BYOND installer put BYOND, from the machine-wide registry (read only): `installpath` under
/// Dantom\BYOND, else the uninstall entry's `InstallLocation`. Both are in the 32-bit view.
pub fn byond_dirs() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RegGetValueW};
    let read = |key: &str, value: &str| -> Option<PathBuf> {
        let (key, value) = (elevate::wide(OsStr::new(key)), elevate::wide(OsStr::new(value)));
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: the key and value names are NUL-terminated; `len` is the buffer's size in bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        let n = (len as usize / 2).saturating_sub(1);
        (rc == 0 && n > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..n])))
    };
    let mut dirs: Vec<PathBuf> = [
        read(r"SOFTWARE\Dantom\BYOND", "installpath"),
        read(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\BYOND", "InstallLocation"),
    ]
    .into_iter()
    .flatten()
    .collect();
    dirs.dedup();
    dirs
}

/// Whether `dir` holds the compiler: the program, or only its `_orig` backup when the patched one was deleted.
fn has_compiler(dir: &Path) -> bool {
    dir.join(COMPILER).is_file() || backup_of(dir, COMPILER).is_file()
}

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let mut dir = if target.is_file() { target.parent().unwrap_or(Path::new(".")).to_path_buf() } else { target.to_path_buf() };
    // The BYOND folder itself (the one holding `bin`) works too.
    if !has_compiler(&dir) && has_compiler(&dir.join("bin")) {
        dir = dir.join("bin");
    }
    if !has_compiler(&dir) {
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

/// What a program is before installing: stock (its bytes become the backup), or already patched (the
/// stock bytes come from the backup; also when the patched program itself was deleted).
enum Before {
    Stock(Vec<u8>),
    Patched(Vec<u8>),
}

/// Reads everything [`patch`] needs, so a program that can't be read stops the install before anything changes.
fn check(dir: &Path, name: &str) -> Result<Before, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if exe.is_file() {
        let data = read(&exe)?;
        if !pe::imports(&data, DLL)? {
            return Ok(Before::Stock(data));
        }
    }
    if !backup.is_file() {
        return Err(format!("{name} has dmfast but {} is missing; reinstall BYOND", backup.display()).into());
    }
    Ok(Before::Patched(read(&backup)?))
}

/// Rewrite one program to import dmfast.dll first, keeping the stock one as its `_orig` backup.
/// Returns what was there before (`stock`, or the dmfast version it had).
fn patch(dir: &Path, name: &str, state: Before, installed: &Option<String>) -> Result<String, Failure> {
    let backup = backup_of(dir, name);
    let (stock, before) = match state {
        Before::Patched(stock) => (stock, installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into())),
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        Before::Stock(data) => {
            let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
            write(&backup, &data)?;
            (data, before.to_string())
        }
    };
    write(&dir.join(name), &pe::add_imports_first(&stock, &[(DLL, "dmfast_mimalloc_version")])?)?;
    Ok(before)
}

enum Restored {
    /// The original is back.
    Yes,
    /// Not patched: left alone.
    NotPatched,
    /// Patched, but the backup to restore is gone.
    NoBackup,
}

/// Put the original back if `name` is patched (or only its backup is left).
fn restore(dir: &Path, name: &str) -> Result<Restored, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if exe.is_file() && !pe::imports(&read(&exe)?, DLL)? {
        return Ok(Restored::NotPatched);
    }
    if !backup.is_file() {
        return Ok(if exe.is_file() { Restored::NoBackup } else { Restored::NotPatched });
    }
    rename_retry(&backup, &exe).map_err(|e| io_err("cannot restore", &exe, e))?;
    Ok(Restored::Yes)
}

pub fn install(dir: &Path, runtime: bool) -> Result<String, Failure> {
    let names = targets(dir, runtime);
    for name in &names {
        let p = dir.join(name);
        if p.is_file() {
            ensure_not_in_use(&p)?;
        }
    }
    let core = dir.join("byondcore.dll");
    let compiler = dir.join(COMPILER);
    if (compiler.is_file() && pe::has_section(&read(&compiler)?, b".dmimp"))
        || (core.is_file() && pe::has_section(&read(&core)?, b".dmfast"))
    {
        return Err("this BYOND has dmfast's old on-disk patches; \
                    put byondcore.dll.orig and dm.exe.orig back first"
            .into());
    }
    let ours = pe::canary(DMFAST_DLL).ok_or("internal: embedded dmfast.dll has no canary")?;
    let dll = dir.join(DLL);
    let installed = dll.is_file().then(|| read(&dll)).transpose()?.and_then(|d| pe::canary(&d));
    let states = names.iter().map(|n| check(dir, n)).collect::<Result<Vec<_>, _>>()?;
    // The DLLs first: a patched program must never import a DLL that isn't there.
    write(&dll, DMFAST_DLL)?;
    for (name, data) in SUPPORT {
        write(&dir.join(name), data)?;
    }
    let mut before = String::new();
    for (name, state) in names.iter().zip(states) {
        let b = patch(dir, name, state, &installed)?;
        if *name == COMPILER {
            before = b;
        }
    }
    // With --no-runtime, a previous runtime install is undone (re-running with it means
    // "compiler only").
    let mut unpatched = Vec::new();
    for name in RUNTIME.into_iter().chain(RETIRED).filter(|n| !names.contains(n)) {
        match restore(dir, name)? {
            Restored::Yes => unpatched.push(name),
            Restored::NotPatched => {}
            Restored::NoBackup => {
                return Err(format!("{name} has dmfast but {} is missing; reinstall BYOND", backup_of(dir, name).display()).into());
            }
        }
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

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let dll = dir.join(DLL);
    let all = std::iter::once(COMPILER).chain(RUNTIME).chain(RETIRED);
    for name in all.clone() {
        let p = dir.join(name);
        if p.is_file() {
            ensure_not_in_use(&p)?;
        }
    }
    // Only for the message: an unreadable DLL must not block removing it.
    let ver = dll.is_file().then(|| read(&dll).ok()).flatten().and_then(|d| pe::canary(&d));
    let (mut restored, mut lost) = (Vec::new(), Vec::new());
    for name in all {
        match restore(dir, name)? {
            Restored::Yes => restored.push(name),
            Restored::NotPatched => {}
            Restored::NoBackup => {
                // It imports a DLL uninstall removes, so it could never start again: remove it so uninstall can finish.
                let exe = dir.join(name);
                fs::remove_file(&exe).map_err(|e| io_err("cannot remove", &exe, e))?;
                lost.push(name);
            }
        }
    }
    if remove_seeker_proxy(dir)? {
        restored.push(LOADER);
    }
    if restored.is_empty() && lost.is_empty() && !dll.is_file() {
        return Ok(None);
    }
    for p in [dll].into_iter().chain(SUPPORT.map(|(name, _)| dir.join(name))).chain(OBSOLETE.map(|name| dir.join(name))) {
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    let mut msg = format!("removed {} from {}", ver.unwrap_or_else(|| "dmfast".into()), dir.display());
    if !restored.is_empty() {
        msg += &format!(" (restored {})", restored.join(", "));
    }
    for name in lost {
        msg += &format!("\nwarning: {} was missing, so {name} is gone; reinstall BYOND to get it back", backup_of(dir, name).display());
    }
    Ok(Some(msg))
}
