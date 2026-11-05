//! Windows: give BYOND programs an extra import, `dmfast.dll`, which patches byondcore.dll in memory and
//! pulls in `mimalloc.dll`, dmfast's allocator (mimalloc for small blocks, the process heap for large
//! ones; source in `vendor/mimalloc-x86`). It goes into the compiler (`dm.exe`) unless `--no-compiler` is given, and
//! DreamDaemon (`dreamdaemon.exe`, `dd.exe`) unless `--no-server` is; each patched program's original is kept as
//! `<name>_orig.exe`, and byondcore.dll and every other program stay stock.

mod elevate;
mod pe;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

pub use elevate::{own_console, pause_if_own_console, relaunch_elevated};

use crate::Installed;
use crate::files::{Failure, io_err, read, rename_retry, write};

const DMFAST_DLL: &[u8] = include_bytes!(env!("DMFAST_LIB"));
/// What dmfast.dll needs next to it: the allocator.
const SUPPORT: [(&str, &[u8]); 1] = [("mimalloc.dll", include_bytes!("../../../vendor/mimalloc-x86/mimalloc.dll"))];
/// Files earlier builds installed next to dmfast.dll that are no longer used; install and uninstall remove them.
const OBSOLETE: [&str; 1] = ["mimalloc-redirect32.dll"];
const DLL: &str = "dmfast.dll";
const COMPILER: &str = "dm.exe";
/// Patched unless `--no-server` is given. Missing ones are skipped (not every BYOND build ships all of them).
const RUNTIME: [&str; 2] = ["dreamdaemon.exe", "dd.exe"];
/// Every program `dmfast.dll` may go into.
const PROGRAMS: [&str; 3] = [COMPILER, RUNTIME[0], RUNTIME[1]];

fn backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(format!("{}_orig.exe", exe.trim_end_matches(".exe")))
}

/// Fails if the program (or a BYOND program using byondcore.dll) is running: images cannot be opened
/// for writing.
fn ensure_not_in_use(p: &Path) -> Result<(), Failure> {
    fs::OpenOptions::new().write(true).open(p).map(drop).map_err(|e| io_err("cannot open", p, e))
}

/// [`ensure_not_in_use`] for each of `names` present in `dir`.
fn ensure_none_in_use(dir: &Path, names: &[&str]) -> Result<(), Failure> {
    for name in names {
        let p = dir.join(name);
        if p.is_file() {
            ensure_not_in_use(&p)?;
        }
    }
    Ok(())
}

fn missing_backup(dir: &Path, name: &str) -> Failure {
    format!("{name} has dmfast but {} is missing; reinstall BYOND", backup_of(dir, name).display()).into()
}

/// For the guided install: what of dmfast `dir` has now.
pub fn state(dir: &Path) -> Installed {
    let patched = |exe: &str| backup_of(dir, exe).is_file();
    Installed {
        any: dir.join(DLL).is_file(),
        compiler: patched(COMPILER),
        server: RUNTIME.iter().any(|exe| patched(exe)),
    }
}

/// Where the BYOND installer put BYOND, from the machine-wide registry (read only): `installpath` under
/// Dantom\BYOND, else the uninstall entry's `InstallLocation`. Both are in the 32-bit view.
pub fn byond_dirs() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RegGetValueW,
    };
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
    let mut dir =
        if target.is_file() { target.parent().unwrap_or(Path::new(".")).to_path_buf() } else { target.to_path_buf() };
    // The BYOND folder itself (the one holding `bin`) works too.
    if !has_compiler(&dir) && has_compiler(&dir.join("bin")) {
        dir = dir.join("bin");
    }
    if !has_compiler(&dir) {
        return Err(format!("no {COMPILER} in {}", dir.display()).into());
    }
    Ok(dir)
}

/// The programs to patch: the compiler and the runtime programs present in `dir`, as asked.
fn targets(dir: &Path, compiler: bool, server: bool) -> Vec<&'static str> {
    let rt = RUNTIME.into_iter().filter(|e| server && dir.join(e).is_file());
    compiler.then_some(COMPILER).into_iter().chain(rt).collect()
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
        return Err(missing_backup(dir, name));
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

/// What [`restore`] would do to a program.
enum Restore {
    /// Patched: its backup goes back.
    Backup,
    /// Not patched: left alone.
    NotPatched,
    /// Patched, but the backup to restore is gone.
    NoBackup,
}

/// Whether `name` is patched (or only its backup is left) and its backup is there. Reads only.
fn restore_check(dir: &Path, name: &str) -> Result<Restore, Failure> {
    let exe = dir.join(name);
    if exe.is_file() && !pe::imports(&read(&exe)?, DLL)? {
        return Ok(Restore::NotPatched);
    }
    if !backup_of(dir, name).is_file() {
        return Ok(if exe.is_file() { Restore::NoBackup } else { Restore::NotPatched });
    }
    Ok(Restore::Backup)
}

/// Put `name`'s original back from its backup.
fn restore(dir: &Path, name: &str) -> Result<(), Failure> {
    let exe = dir.join(name);
    rename_retry(&backup_of(dir, name), &exe).map_err(|e| io_err("cannot restore", &exe, e))
}

pub fn install(dir: &Path, compiler: bool, server: bool) -> Result<String, Failure> {
    let names = targets(dir, compiler, server);
    if names.is_empty() {
        return Err(format!("nothing to install in {}: no {}", dir.display(), RUNTIME.join(" or ")).into());
    }
    // A program left out is put back (re-running without it means "not this one").
    let mut left_out = Vec::new();
    for name in PROGRAMS.into_iter().filter(|n| !names.contains(n)) {
        match restore_check(dir, name)? {
            Restore::Backup => left_out.push(name),
            Restore::NotPatched => {}
            Restore::NoBackup => return Err(missing_backup(dir, name)),
        }
    }
    ensure_none_in_use(dir, &names)?;
    ensure_none_in_use(dir, &left_out)?;
    let core = dir.join("byondcore.dll");
    let dm = dir.join(COMPILER);
    if (dm.is_file() && pe::has_section(&read(&dm)?, b".dmimp"))
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
    let mut before = None;
    for (name, state) in names.iter().zip(states) {
        let b = patch(dir, name, state, &installed)?;
        before.get_or_insert(b);
    }
    for name in &left_out {
        restore(dir, name)?;
    }
    for name in OBSOLETE {
        let p = dir.join(name);
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    let before = before.unwrap_or_default();
    let mut msg = format!("installed {ours} in {} (was {before}) for {}", dir.display(), names.join(", "));
    if !left_out.is_empty() {
        msg += &format!("; restored stock {}", left_out.join(", "));
    }
    Ok(msg)
}

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let dll = dir.join(DLL);
    ensure_none_in_use(dir, &PROGRAMS)?;
    // Only for the message: an unreadable DLL must not block removing it.
    let ver = dll.is_file().then(|| read(&dll).ok()).flatten().and_then(|d| pe::canary(&d));
    let (mut restored, mut lost) = (Vec::new(), Vec::new());
    for name in PROGRAMS {
        match restore_check(dir, name)? {
            Restore::Backup => {
                restore(dir, name)?;
                restored.push(name);
            }
            Restore::NotPatched => {}
            Restore::NoBackup => {
                // It imports a DLL uninstall removes, so it could never start again: remove it so uninstall can finish.
                let exe = dir.join(name);
                fs::remove_file(&exe).map_err(|e| io_err("cannot remove", &exe, e))?;
                lost.push(name);
            }
        }
    }
    if restored.is_empty() && lost.is_empty() && !dll.is_file() {
        return Ok(None);
    }
    for p in [dll].into_iter().chain(SUPPORT.map(|(name, _)| dir.join(name))).chain(OBSOLETE.map(|name| dir.join(name)))
    {
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    let mut msg = format!("removed {} from {}", ver.unwrap_or_else(|| "dmfast".into()), dir.display());
    if !restored.is_empty() {
        msg += &format!(" (restored {})", restored.join(", "));
    }
    for name in lost {
        msg += &format!(
            "\nwarning: {} was missing, so {name} is gone; reinstall BYOND to get it back",
            backup_of(dir, name).display()
        );
    }
    Ok(Some(msg))
}
