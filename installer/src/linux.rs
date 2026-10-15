//! Linux: write `libdmfast.so` and replace BYOND programs with a small sh wrapper that runs the stock
//! binary (kept as `<name>_orig`) with the library in `LD_PRELOAD`. `DreamMaker` is always wrapped, and
//! `DreamDaemon` unless `--no-runtime` is given. libbyond.so and every other program stay stock. BYOND's own
//! `byondexec` launcher runs the wrappers like any program.

use std::fs;
use std::path::{Path, PathBuf};

use object::{Object, ObjectSection, ObjectSymbol};

use crate::files::{Failure, io_err, read, rename_retry, write, write_executable};

const DMFAST_SO: &[u8] = include_bytes!(env!("DMFAST_LIB"));
const LIB: &str = "libdmfast.so";
const COMPILER: &str = "DreamMaker";
/// Wrapped unless `--no-runtime` is given.
const RUNTIME: [&str; 1] = ["DreamDaemon"];
/// First lines of the wrapper; how an installed program is recognised.
const WRAPPER_TAG: &str = "#!/bin/sh\n# dmfast:";

fn backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(format!("{exe}_orig"))
}

fn wrapper(version: &str, exe: &str) -> String {
    format!(
        "{WRAPPER_TAG} {version} runs the stock {exe} ({exe}_orig) with {LIB} preloaded.\n\
         # Remove with: dmfast-install --uninstall <this folder>\n\
         dir=$(dirname \"$(readlink -f \"$0\")\")\n\
         LD_PRELOAD=\"$dir/{LIB}${{LD_PRELOAD:+ $LD_PRELOAD}}\" exec \"$dir/{exe}_orig\" \"$@\"\n"
    )
}

/// The dmfast canary (`DMFAST <version>`) exported by a libdmfast.so.
fn canary(data: &[u8]) -> Option<String> {
    let file = object::File::parse(data).ok()?;
    let sym = file.dynamic_symbols().find(|s| s.name() == Ok("dmfast_canary"))?;
    let section = file.section_by_index(sym.section_index()?).ok()?;
    let bytes = section.data_range(sym.address(), sym.size()).ok()??;
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.trim_end_matches('\0').to_string())
}

fn installed_canary(lib: &Path) -> Result<Option<String>, Failure> {
    Ok(if lib.is_file() { canary(&read(lib)?) } else { None })
}

fn is_wrapper(data: &[u8]) -> bool {
    data.starts_with(WRAPPER_TAG.as_bytes())
}

/// For the guided install: whether dmfast is installed in `dir`, and whether DreamDaemon is wrapped.
pub fn state(dir: &Path) -> (bool, bool) {
    (dir.join(LIB).is_file(), RUNTIME.iter().any(|exe| backup_of(dir, exe).is_file()))
}

/// The usual BYOND folders (BYOND's Linux zip has no installer to record where it went).
pub fn byond_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("byond"));
    home.into_iter().chain([PathBuf::from("/usr/local/byond")]).filter(|d| bin_dir(d).is_ok()).collect()
}

/// Whether `dir` holds the compiler: the program, or only its `_orig` backup when the wrapper was deleted.
fn has_compiler(dir: &Path) -> bool {
    dir.join(COMPILER).is_file() || backup_of(dir, COMPILER).is_file()
}

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let mut dir = if target.is_file() { target.parent().unwrap_or(Path::new(".")).to_path_buf() } else { target.to_path_buf() };
    // The BYOND folder itself (the one holding `bin`) works too.
    if !has_compiler(&dir) && has_compiler(&dir.join("bin")) {
        dir = dir.join("bin");
    }
    if !has_compiler(&dir) || !dir.join("libbyond.so").is_file() {
        return Err(format!("no {COMPILER} and libbyond.so in {}", dir.display()).into());
    }
    fs::canonicalize(&dir).map_err(|e| io_err("cannot resolve", &dir, e))
}

/// What a program is before installing: stock (its bytes become the backup), or already ours with a
/// readable backup (also when the wrapper itself was deleted).
enum Before {
    Stock(Vec<u8>),
    Wrapped,
}

/// Reads everything [`wrap`] needs, so a program that can't be read stops the install before anything changes.
fn check(dir: &Path, name: &str) -> Result<Before, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if exe.is_file() {
        let data = read(&exe)?;
        if !is_wrapper(&data) {
            return Ok(Before::Stock(data));
        }
    }
    if !backup.is_file() {
        return Err(format!("{name} is the dmfast wrapper but {name}_orig is missing; reinstall BYOND").into());
    }
    fs::File::open(&backup).map_err(|e| io_err("cannot read", &backup, e))?;
    Ok(Before::Wrapped)
}

/// Wrap one program, keeping the stock binary as its `_orig` backup. Returns what was there before.
fn wrap(dir: &Path, name: &str, state: Before, ours: &str, installed: &Option<String>) -> Result<String, Failure> {
    let before = match state {
        Before::Wrapped => installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into()),
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        Before::Stock(data) => {
            let before = if backup_of(dir, name).is_file() { "stock (BYOND was updated)" } else { "stock" };
            write_executable(&backup_of(dir, name), &data)?;
            before.to_string()
        }
    };
    write_executable(&dir.join(name), wrapper(ours, name).as_bytes())?;
    Ok(before)
}

enum Unwrapped {
    /// The stock program is back.
    Restored,
    /// Not ours: left alone.
    NotWrapped,
    /// Our wrapper, but the backup to restore is gone.
    NoBackup,
}

/// Put the stock binary back if `name` is wrapped (or only its backup is left).
fn unwrap(dir: &Path, name: &str) -> Result<Unwrapped, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if exe.is_file() && !is_wrapper(&read(&exe)?) {
        return Ok(Unwrapped::NotWrapped);
    }
    if !backup.is_file() {
        return Ok(if exe.is_file() { Unwrapped::NoBackup } else { Unwrapped::NotWrapped });
    }
    rename_retry(&backup, &exe).map_err(|e| io_err("cannot restore", &exe, e))?;
    Ok(Unwrapped::Restored)
}

pub fn install(dir: &Path, runtime: bool) -> Result<String, Failure> {
    // LD_PRELOAD entries are separated by spaces and colons; the wrapper passes the full path.
    if dir.to_string_lossy().contains([' ', ':']) {
        return Err(format!("{} contains a space or colon, which LD_PRELOAD cannot express", dir.display()).into());
    }
    let names: Vec<&str> =
        std::iter::once(COMPILER).chain(RUNTIME.into_iter().filter(|e| runtime && dir.join(e).is_file())).collect();
    let lib = dir.join(LIB);
    let ours = canary(DMFAST_SO).ok_or("internal: embedded libdmfast.so has no canary")?;
    let installed = installed_canary(&lib)?;
    let states = names.iter().map(|n| check(dir, n)).collect::<Result<Vec<_>, _>>()?;
    // The library first: a wrapper must never point at a library that isn't there.
    write(&lib, DMFAST_SO)?;
    let mut before = String::new();
    for (name, state) in names.iter().zip(states) {
        let b = wrap(dir, name, state, &ours, &installed)?;
        if *name == COMPILER {
            before = b;
        }
    }
    // With --no-runtime, a previous runtime install is undone.
    let mut unwrapped = Vec::new();
    for name in RUNTIME.into_iter().filter(|n| !names.contains(n)) {
        match unwrap(dir, name)? {
            Unwrapped::Restored => unwrapped.push(name),
            Unwrapped::NotWrapped => {}
            Unwrapped::NoBackup => {
                return Err(format!("{name} is the dmfast wrapper but {name}_orig is missing; reinstall BYOND").into());
            }
        }
    }
    let mut msg = format!("installed {ours} in {} (was {before}) for {}", dir.display(), names.join(", "));
    if !unwrapped.is_empty() {
        msg += &format!("; restored stock {}", unwrapped.join(", "));
    }
    Ok(msg)
}

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let lib = dir.join(LIB);
    // Only for the message: an unreadable library must not block removing it.
    let ver = installed_canary(&lib).ok().flatten();
    let (mut restored, mut lost) = (Vec::new(), Vec::new());
    for name in std::iter::once(COMPILER).chain(RUNTIME) {
        match unwrap(dir, name)? {
            Unwrapped::Restored => restored.push(name),
            Unwrapped::NotWrapped => {}
            Unwrapped::NoBackup => {
                // The wrapper only runs the missing backup: remove it so uninstall can finish.
                let exe = dir.join(name);
                fs::remove_file(&exe).map_err(|e| io_err("cannot remove", &exe, e))?;
                lost.push(name);
            }
        }
    }
    if restored.is_empty() && lost.is_empty() && !lib.is_file() {
        return Ok(None);
    }
    if lib.is_file() {
        fs::remove_file(&lib).map_err(|e| io_err("cannot remove", &lib, e))?;
    }
    let mut msg = format!("removed {} from {}", ver.unwrap_or_else(|| "dmfast".into()), dir.display());
    if !restored.is_empty() {
        msg += &format!(" (restored {})", restored.join(", "));
    }
    for name in lost {
        msg += &format!("\nwarning: {name}_orig was missing, so {name} is gone; reinstall BYOND to get it back");
    }
    Ok(Some(msg))
}
