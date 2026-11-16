//! Windows: give BYOND programs an extra import, `dmfast.dll`, which patches byondcore.dll in memory and
//! pulls in `mimalloc.dll`, dmfast's allocator (mimalloc for small blocks, the process heap for large
//! ones; source in `vendor/mimalloc-x86`). It goes into the compiler (`dm.exe`) unless `--no-compiler` is given, and
//! DreamDaemon (`dreamdaemon.exe`, `dd.exe`) unless `--no-server` is, and every other program stays stock. With
//! `--client` (saved like the fixes), byondcore.dll and byondwin.dll also get the DreamSeeker patches it picks, on disk
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
use crate::backup::{self, Before, Programs, backup_of};
use crate::files::{Failure, io_err, read, write};

const DMFAST_DLL: &[u8] = include_bytes!(env!("DMFAST_LIB"));
/// What dmfast.dll needs next to it: the allocator.
const SUPPORT: [(&str, &[u8]); 1] = [("mimalloc.dll", include_bytes!("../../../vendor/mimalloc-x86/mimalloc.dll"))];
/// Files earlier builds installed next to dmfast.dll that are no longer used; install and uninstall remove them.
const OBSOLETE: [&str; 1] = ["mimalloc-redirect32.dll"];
const DLL: &str = "dmfast.dll";
const COMPILER: &str = "dm.exe";
/// Patched unless `--no-server` is given. Missing ones are skipped (not every BYOND build ships all of them).
const RUNTIME: [&str; 2] = ["dreamdaemon.exe", "dd.exe"];
/// Every program `dmfast.dll` may go into, and how to tell it is in one.
const PROGRAMS: Programs = Programs {
    all: &[COMPILER, RUNTIME[0], RUNTIME[1]],
    old_backup_of: |dir, exe| dir.join(format!("{}_orig.exe", exe.trim_end_matches(".exe"))),
    has_dmfast: |data| Ok(pe::imports(data, DLL)?),
    has: "has dmfast",
    shown_backup: |dir, name| backup_of(dir, name).display().to_string(),
};

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

/// For the guided install: what of dmfast `dir` has now.
pub fn state(dir: &Path) -> Installed {
    let patched = |exe: &str| PROGRAMS.has_stock_copy(dir, exe);
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

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let dir = PROGRAMS.bin_dir(target);
    if !PROGRAMS.has_compiler(&dir) {
        return Err(format!("no {COMPILER} in {}", dir.display()).into());
    }
    Ok(dir)
}

/// The programs to patch: the compiler and the runtime programs present in `dir`, as asked.
fn targets(dir: &Path, compiler: bool, server: bool) -> Vec<&'static str> {
    let rt = RUNTIME.into_iter().filter(|e| server && dir.join(e).is_file());
    compiler.then_some(COMPILER).into_iter().chain(rt).collect()
}

/// Rewrite one program to import dmfast.dll first, keeping the stock one in `backup/`.
/// Returns what was there before (`stock`, or the dmfast version it had).
fn patch(dir: &Path, name: &str, state: Before<Vec<u8>>, installed: &Option<String>) -> Result<String, Failure> {
    let before = backup::keep_stock(dir, name, &state, installed, write)?;
    let (Before::Stock(stock) | Before::Ours(stock)) = state;
    write(&dir.join(name), &pe::add_imports_first(&stock, &[(DLL, "dmfast_mimalloc_version")])?)?;
    Ok(before)
}

pub fn install(dir: &Path, compiler: bool, server: bool, client: &str) -> Result<String, Failure> {
    let names = targets(dir, compiler, server);
    PROGRAMS.migrate(dir)?;
    let left_out = PROGRAMS.left_out(dir, &names)?;
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
    let states = names.iter().map(|n| PROGRAMS.check(dir, n, read)).collect::<Result<Vec<_>, _>>()?;
    let seeker = seeker::plan_install(dir, client)?;
    if names.is_empty() && !seeker.patched {
        let mut why = Vec::new();
        if server {
            why.push(format!("no {}", RUNTIME.join(" or ")));
        }
        if !seeker::selection(client)?.is_empty() {
            why.extend(seeker.lines().iter().cloned());
        }
        if why.is_empty() {
            why.push("the compiler, DreamDaemon, and DreamSeeker are all left out".into());
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
        PROGRAMS.restore(dir, name)?;
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
    let mut parts = PROGRAMS.parts(&names);
    if seeker.patched {
        parts.push("client");
    }
    let mut msg = PROGRAMS.installed_message(dir, &ours, before, &parts, &left_out);
    for line in seeker::carry_out(dir, seeker)? {
        msg += &format!("\n{line}");
    }
    Ok(msg)
}

pub use seeker::{
    audit, client_list, current as current_client, has_client_patches, selection as client_selection, set as set_client,
};

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let dll = dir.join(DLL);
    ensure_none_in_use(dir, PROGRAMS.all)?;
    PROGRAMS.migrate(dir)?;
    let seeker = seeker::plan_uninstall(dir)?;
    // Only for the message: an unreadable DLL must not block removing it.
    let ver = dll.is_file().then(|| read(&dll).ok()).flatten().and_then(|d| pe::canary(&d));
    let (restored, lost) = PROGRAMS.restore_all(dir)?;
    if restored.is_empty() && lost.is_empty() && !dll.is_file() && seeker.is_empty() {
        return Ok(None);
    }
    for p in [dll].into_iter().chain(SUPPORT.map(|(name, _)| dir.join(name))).chain(OBSOLETE.map(|name| dir.join(name)))
    {
        if p.is_file() {
            fs::remove_file(&p).map_err(|e| io_err("cannot remove", &p, e))?;
        }
    }
    let mut msg = PROGRAMS.removed_message(dir, ver, &restored, &lost);
    for line in seeker::carry_out(dir, seeker)? {
        msg += &format!("\n{line}");
    }
    Ok(Some(msg))
}
