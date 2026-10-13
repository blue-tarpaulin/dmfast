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

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let dir = if target.is_file() { target.parent().unwrap_or(Path::new(".")).to_path_buf() } else { target.to_path_buf() };
    if !dir.join(COMPILER).is_file() || !dir.join("libbyond.so").is_file() {
        return Err(format!("no {COMPILER} and libbyond.so in {}", dir.display()).into());
    }
    fs::canonicalize(&dir).map_err(|e| io_err("cannot resolve", &dir, e))
}

/// Wrap one program, keeping the stock binary as its `_orig` backup. Returns what was there before.
fn wrap(dir: &Path, name: &str, ours: &str, installed: &Option<String>) -> Result<String, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    let exe_data = read(&exe)?;
    let before = if is_wrapper(&exe_data) {
        if !backup.is_file() {
            return Err(format!("{name} is the dmfast wrapper but {name}_orig is missing; reinstall BYOND").into());
        }
        installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into())
    } else {
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
        write_executable(&backup, &exe_data)?;
        before.to_string()
    };
    write_executable(&exe, wrapper(ours, name).as_bytes())?;
    Ok(before)
}

/// Put the stock binary back if `name` is wrapped. Returns whether it was.
fn unwrap(dir: &Path, name: &str) -> Result<bool, Failure> {
    let exe = dir.join(name);
    let backup = backup_of(dir, name);
    if !exe.is_file() || !is_wrapper(&read(&exe)?) {
        return Ok(false);
    }
    if !backup.is_file() {
        return Err(format!("{name}_orig is missing; reinstall BYOND to restore {name}").into());
    }
    rename_retry(&backup, &exe).map_err(|e| io_err("cannot restore", &exe, e))?;
    Ok(true)
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
    write(&lib, DMFAST_SO)?;
    let mut before = String::new();
    for name in &names {
        let b = wrap(dir, name, &ours, &installed)?;
        if *name == COMPILER {
            before = b;
        }
    }
    // With --no-runtime, a previous runtime install is undone.
    let mut unwrapped = Vec::new();
    for name in RUNTIME.into_iter().filter(|n| !names.contains(n)) {
        if unwrap(dir, name)? {
            unwrapped.push(name);
        }
    }
    let mut msg = format!("installed {ours} in {} (was {before}) for {}", dir.display(), names.join(", "));
    if !unwrapped.is_empty() {
        msg += &format!("; restored stock {}", unwrapped.join(", "));
    }
    Ok(msg)
}

pub fn uninstall(dir: &Path) -> Result<String, Failure> {
    let lib = dir.join(LIB);
    let ver = installed_canary(&lib)?;
    let mut restored = Vec::new();
    for name in std::iter::once(COMPILER).chain(RUNTIME) {
        if unwrap(dir, name)? {
            restored.push(name);
        }
    }
    if restored.is_empty() {
        return Ok(format!("dmfast is not installed in {}", dir.display()));
    }
    if lib.is_file() {
        fs::remove_file(&lib).map_err(|e| io_err("cannot remove", &lib, e))?;
    }
    Ok(format!("removed {} from {} (restored {})", ver.unwrap_or_else(|| "dmfast".into()), dir.display(), restored.join(", ")))
}
