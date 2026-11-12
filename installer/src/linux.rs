//! Linux: write `libdmfast.so` and replace BYOND programs with a small sh wrapper that runs the stock
//! binary (kept in `bin/backup/`) with the library in `LD_PRELOAD`. `DreamMaker` is wrapped unless `--no-compiler`
//! is given, and `DreamDaemon` unless `--no-server` is. libbyond.so and every other program stay stock. BYOND's own
//! `byondexec` launcher runs the wrappers like any program.

use std::fs;
use std::path::{Path, PathBuf};

use object::{Object, ObjectSection, ObjectSymbol};

use crate::Installed;
use crate::files::{Failure, io_err, read, rename_retry, write, write_executable};

const DMFAST_SO: &[u8] = include_bytes!(env!("DMFAST_LIB"));
const LIB: &str = "libdmfast.so";
const COMPILER: &str = "DreamMaker";
/// Wrapped unless `--no-server` is given.
const RUNTIME: [&str; 1] = ["DreamDaemon"];
/// Every program the installer may wrap.
const PROGRAMS: [&str; 2] = [COMPILER, RUNTIME[0]];
/// First lines of the wrapper; how an installed program is recognised.
const WRAPPER_TAG: &str = "#!/bin/sh\n# dmfast:";

/// The folder in `bin` holding the stock copy of every program dmfast wraps (and nothing else).
const BACKUP: &str = "backup";

fn backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(BACKUP).join(exe)
}

/// Where earlier builds kept a program's stock copy; [`migrate`] moves it to `backup/`.
fn old_backup_of(dir: &Path, exe: &str) -> PathBuf {
    dir.join(format!("{exe}_orig"))
}

fn has_stock_copy(dir: &Path, exe: &str) -> bool {
    backup_of(dir, exe).is_file() || old_backup_of(dir, exe).is_file()
}

fn make_backup_dir(dir: &Path) -> Result<(), Failure> {
    let folder = dir.join(BACKUP);
    fs::create_dir_all(&folder).map_err(|e| io_err("cannot create", &folder, e))
}

/// Moves each stock copy from where earlier builds kept it into `backup/`.
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

/// Removes `backup/` once nothing is left in it.
fn drop_empty_backup_dir(dir: &Path) -> Result<(), Failure> {
    let folder = dir.join(BACKUP);
    if fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none()) {
        fs::remove_dir(&folder).map_err(|e| io_err("cannot remove", &folder, e))?;
    }
    Ok(())
}

fn wrapper(version: &str, exe: &str) -> String {
    format!(
        "{WRAPPER_TAG} {version} runs the stock {exe} ({BACKUP}/{exe}) with {LIB} preloaded.\n\
         # Remove with: dmfast-install --uninstall <this folder>\n\
         dir=$(dirname \"$(readlink -f \"$0\")\")\n\
         LD_PRELOAD=\"$dir/{LIB}${{LD_PRELOAD:+ $LD_PRELOAD}}\" exec \"$dir/{BACKUP}/{exe}\" \"$@\"\n"
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

/// For the guided install: what of dmfast `dir` has now.
pub fn state(dir: &Path) -> Installed {
    let wrapped = |exe: &str| has_stock_copy(dir, exe);
    Installed {
        any: dir.join(LIB).is_file(),
        compiler: wrapped(COMPILER),
        server: RUNTIME.iter().any(|exe| wrapped(exe)),
    }
}

/// The usual BYOND folders (BYOND's Linux zip has no installer to record where it went).
pub fn byond_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("byond"));
    home.into_iter().chain([PathBuf::from("/usr/local/byond")]).filter(|d| bin_dir(d).is_ok()).collect()
}

/// Whether `dir` holds the compiler: the program, or only its stock copy when the wrapper was deleted.
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
    if exe.is_file() {
        let data = read(&exe)?;
        if !is_wrapper(&data) {
            return Ok(Before::Stock(data));
        }
    }
    let backup = backup_of(dir, name);
    if !backup.is_file() {
        return Err(missing_backup(name));
    }
    fs::File::open(&backup).map_err(|e| io_err("cannot read", &backup, e))?;
    Ok(Before::Wrapped)
}

/// Wrap one program, keeping the stock binary in `backup/`. Returns what was there before.
fn wrap(dir: &Path, name: &str, state: Before, ours: &str, installed: &Option<String>) -> Result<String, Failure> {
    let before = match state {
        Before::Wrapped => installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into()),
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        Before::Stock(data) => {
            let backup = backup_of(dir, name);
            let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
            make_backup_dir(dir)?;
            write_executable(&backup, &data)?;
            before.to_string()
        }
    };
    write_executable(&dir.join(name), wrapper(ours, name).as_bytes())?;
    Ok(before)
}

fn missing_backup(name: &str) -> Failure {
    format!("{name} is the dmfast wrapper but {BACKUP}/{name} is missing; reinstall BYOND").into()
}

/// What [`unwrap`] would do to a program.
enum Unwrap {
    /// Ours: the stock program goes back.
    Backup,
    /// Not ours: left alone.
    NotWrapped,
    /// Our wrapper, but the backup to restore is gone.
    NoBackup,
}

/// Whether `name` is wrapped (or only its backup is left) and its backup is there. Reads only.
fn unwrap_check(dir: &Path, name: &str) -> Result<Unwrap, Failure> {
    let exe = dir.join(name);
    if exe.is_file() && !is_wrapper(&read(&exe)?) {
        return Ok(Unwrap::NotWrapped);
    }
    if !backup_of(dir, name).is_file() {
        return Ok(if exe.is_file() { Unwrap::NoBackup } else { Unwrap::NotWrapped });
    }
    Ok(Unwrap::Backup)
}

/// Put `name`'s stock binary back from its backup.
fn unwrap(dir: &Path, name: &str) -> Result<(), Failure> {
    let exe = dir.join(name);
    rename_retry(&backup_of(dir, name), &exe).map_err(|e| io_err("cannot restore", &exe, e))
}

pub fn install(dir: &Path, compiler: bool, server: bool) -> Result<String, Failure> {
    // LD_PRELOAD entries are separated by spaces and colons; the wrapper passes the full path.
    if dir.to_string_lossy().contains([' ', ':']) {
        return Err(format!("{} contains a space or colon, which LD_PRELOAD cannot express", dir.display()).into());
    }
    let runtime = RUNTIME.into_iter().filter(|e| server && dir.join(e).is_file());
    let names: Vec<&str> = compiler.then_some(COMPILER).into_iter().chain(runtime).collect();
    if names.is_empty() {
        return Err(format!("nothing to install in {}: no {}", dir.display(), RUNTIME.join(" or ")).into());
    }
    migrate(dir)?;
    // A program left out is put back (re-running without it means "not this one").
    let mut left_out = Vec::new();
    for name in PROGRAMS.into_iter().filter(|n| !names.contains(n)) {
        match unwrap_check(dir, name)? {
            Unwrap::Backup => left_out.push(name),
            Unwrap::NotWrapped => {}
            Unwrap::NoBackup => return Err(missing_backup(name)),
        }
    }
    let lib = dir.join(LIB);
    let ours = canary(DMFAST_SO).ok_or("internal: embedded libdmfast.so has no canary")?;
    let installed = installed_canary(&lib)?;
    let states = names.iter().map(|n| check(dir, n)).collect::<Result<Vec<_>, _>>()?;
    // The library first: a wrapper must never point at a library that isn't there.
    write(&lib, DMFAST_SO)?;
    let mut before = None;
    for (name, state) in names.iter().zip(states) {
        let b = wrap(dir, name, state, &ours, &installed)?;
        before.get_or_insert(b);
    }
    for name in &left_out {
        unwrap(dir, name)?;
    }
    drop_empty_backup_dir(dir)?;
    let before = before.unwrap_or_default();
    let mut msg = format!("installed {ours} in {} (was {before}) for {}", dir.display(), names.join(", "));
    if !left_out.is_empty() {
        msg += &format!("; restored stock {}", left_out.join(", "));
    }
    Ok(msg)
}

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let lib = dir.join(LIB);
    // Only for the message: an unreadable library must not block removing it.
    let ver = installed_canary(&lib).ok().flatten();
    migrate(dir)?;
    let (mut restored, mut lost) = (Vec::new(), Vec::new());
    for name in PROGRAMS {
        match unwrap_check(dir, name)? {
            Unwrap::Backup => {
                unwrap(dir, name)?;
                restored.push(name);
            }
            Unwrap::NotWrapped => {}
            Unwrap::NoBackup => {
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
    drop_empty_backup_dir(dir)?;
    let mut msg = format!("removed {} from {}", ver.unwrap_or_else(|| "dmfast".into()), dir.display());
    if !restored.is_empty() {
        msg += &format!(" (restored {})", restored.join(", "));
    }
    for name in lost {
        msg += &format!("\nwarning: {BACKUP}/{name} was missing, so {name} is gone; reinstall BYOND to get it back");
    }
    Ok(Some(msg))
}
