//! Windows: give BYOND programs an extra import, `dmfast.dll`, which patches byondcore.dll in memory and
//! pulls in `mimalloc.dll`, dmfast's allocator (mimalloc for small blocks, the process heap for large
//! ones; source in `vendor/mimalloc-x86`). It goes into the compiler (`dm.exe`) unless `--no-compiler` is given, and
//! DreamDaemon (`dreamdaemon.exe`, `dd.exe`) unless `--no-server` is, and every other program stays stock. With
//! `--client`, byondcore.dll and byondwin.dll also get the DreamSeeker patches the fix selection picks, on disk
//! (`seeker`). The stock copy of every file dmfast changes is kept in `bin/backup/`.

mod elevate;
mod pe;
mod seeker;

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

/// The folder in `bin` holding the stock copy of every file dmfast changed (and nothing else).
const BACKUP: &str = "backup";

fn backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(BACKUP).join(exe)
}

/// Where earlier builds kept a program's stock copy; [`migrate`] moves it to `backup/`.
fn old_backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(format!("{}_orig.exe", exe.trim_end_matches(".exe")))
}

fn has_stock_copy(dir: &Path, exe: &str) -> bool {
    backup_of(dir, exe).is_file() || old_backup_of(dir, exe).is_file()
}

fn make_backup_dir(dir: &Path) -> Result<(), Failure> {
    let folder = dir.join(BACKUP);
    fs::create_dir_all(&folder).map_err(|e| io_err("cannot create", &folder, e))
}

/// Moves each program's stock copy from where earlier builds kept it into `backup/`.
fn migrate(dir: &Path) -> Result<(), Failure> {
    for name in PROGRAMS {
        let old = old_backup_of(dir, name);
        if !old.is_file() {
            continue;
        }
        let backup = backup_of(dir, name);
        if backup.is_file() {
            fs::remove_file(&old).map_err(|e| io_err("cannot remove", &old, e))?;
        } else {
            make_backup_dir(dir)?;
            rename_retry(&old, &backup).map_err(|e| io_err("cannot move", &old, e))?;
        }
    }
    Ok(())
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
    let patched = |exe: &str| has_stock_copy(dir, exe);
    let client = seeker::client_patched(dir);
    Installed {
        any: dir.join(DLL).is_file() || client,
        compiler: patched(COMPILER),
        server: RUNTIME.iter().any(|exe| patched(exe)),
        client,
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

/// Whether `dir` holds the compiler: the program, or only its stock copy when the patched one was deleted.
fn has_compiler(dir: &Path) -> bool {
    dir.join(COMPILER).is_file() || has_stock_copy(dir, COMPILER)
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
    if exe.is_file() {
        let data = read(&exe)?;
        if !pe::imports(&data, DLL)? {
            return Ok(Before::Stock(data));
        }
    }
    let backup = backup_of(dir, name);
    if !backup.is_file() {
        return Err(missing_backup(dir, name));
    }
    Ok(Before::Patched(read(&backup)?))
}

/// Rewrite one program to import dmfast.dll first, keeping the stock one in `backup/`.
/// Returns what was there before (`stock`, or the dmfast version it had).
fn patch(dir: &Path, name: &str, state: Before, installed: &Option<String>) -> Result<String, Failure> {
    let backup = backup_of(dir, name);
    let (stock, before) = match state {
        Before::Patched(stock) => (stock, installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into())),
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        Before::Stock(data) => {
            let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
            make_backup_dir(dir)?;
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

pub fn install(dir: &Path, compiler: bool, server: bool, client: bool) -> Result<String, Failure> {
    let names = targets(dir, compiler, server);
    migrate(dir)?;
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
    let seeker = seeker::plan_install(dir, client)?;
    if names.is_empty() && !seeker.patched {
        // Without the compiler, `server` or `client` was asked for.
        let mut why = Vec::new();
        if server {
            why.push(format!("no {}", RUNTIME.join(" or ")));
        }
        if client {
            why.extend(seeker.lines().iter().cloned());
        }
        if why.is_empty() {
            why.push("no DreamSeeker DLL to patch".into());
        }
        return Err(format!("nothing to install in {}: {}", dir.display(), why.join(", and ")).into());
    }
    // The DLLs first: a patched program must never import a DLL that isn't there.
    if !names.is_empty() {
        write(&dll, DMFAST_DLL)?;
        for (name, data) in SUPPORT {
            write(&dir.join(name), data)?;
        }
    }
    let mut before = None;
    for (name, state) in names.iter().zip(states) {
        let b = patch(dir, name, state, &installed)?;
        before.get_or_insert(b);
    }
    for name in &left_out {
        restore(dir, name)?;
    }
    // No program imports dmfast.dll any more: it and what it needs go too.
    let unused =
        if names.is_empty() { [DLL].into_iter().chain(SUPPORT.map(|(name, _)| name)).collect() } else { vec![] };
    for name in OBSOLETE.into_iter().chain(unused) {
        let p = dir.join(name);
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    let mut msg = match before {
        Some(before) => format!("installed {ours} in {} (was {before}) for {}", dir.display(), names.join(", ")),
        None => format!("DreamSeeker patched in {}", dir.display()),
    };
    if !left_out.is_empty() {
        msg += &format!("; restored stock {}", left_out.join(", "));
    }
    for line in seeker::carry_out(dir, seeker)? {
        msg += &format!("\n{line}");
    }
    Ok(msg)
}

pub use seeker::{audit, client_list, has_client_patches};

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let dll = dir.join(DLL);
    ensure_none_in_use(dir, &PROGRAMS)?;
    migrate(dir)?;
    let seeker = seeker::plan_uninstall(dir)?;
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
    if restored.is_empty() && lost.is_empty() && !dll.is_file() && seeker.is_empty() {
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
    for line in seeker::carry_out(dir, seeker)? {
        msg += &format!("\n{line}");
    }
    Ok(Some(msg))
}
