//! Linux: write `libdmfast.so` and replace BYOND programs with a small sh wrapper that runs the stock
//! binary (kept in `bin/backup/`) with the library in `LD_PRELOAD`. `DreamMaker` is wrapped unless `--no-compiler`
//! is given, and `DreamDaemon` unless `--no-server` is. libbyond.so and every other program stay stock. BYOND's own
//! `byondexec` launcher runs the wrappers like any program.

use std::fs;
use std::path::{Path, PathBuf};

use object::{Object, ObjectSection, ObjectSymbol};

use crate::Installed;
use crate::backup::{self, BACKUP, Before, Programs, drop_empty_backup_dir};
use crate::files::{Failure, io_err, read, write, write_executable};

const DMFAST_SO: &[u8] = include_bytes!(env!("DMFAST_LIB"));
const LIB: &str = "libdmfast.so";
const COMPILER: &str = "DreamMaker";
/// Wrapped unless `--no-server` is given.
const RUNTIME: [&str; 1] = ["DreamDaemon"];
/// First lines of the wrapper; how an installed program is recognised.
const WRAPPER_TAG: &str = "#!/bin/sh\n# dmfast:";
/// Every program the installer may wrap, and how to tell it is wrapped.
const PROGRAMS: Programs = Programs {
    all: &[COMPILER, RUNTIME[0]],
    old_backup_of: |dir, exe| dir.join(format!("{exe}_orig")),
    has_dmfast: |data| Ok(data.starts_with(WRAPPER_TAG.as_bytes())),
    has: "is the dmfast wrapper",
    shown_backup: |_, name| format!("{BACKUP}/{name}"),
};

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

/// For the guided install: what of dmfast `dir` has now.
pub fn state(dir: &Path) -> Installed {
    let wrapped = |exe: &str| PROGRAMS.has_stock_copy(dir, exe);
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

pub fn bin_dir(target: &Path) -> Result<PathBuf, Failure> {
    let dir = PROGRAMS.bin_dir(target);
    if !PROGRAMS.has_compiler(&dir) || !dir.join("libbyond.so").is_file() {
        return Err(format!("no {COMPILER} and libbyond.so in {}", dir.display()).into());
    }
    fs::canonicalize(&dir).map_err(|e| io_err("cannot resolve", &dir, e))
}

/// Wrap one program, keeping the stock binary in `backup/`. Returns what was there before.
fn wrap(dir: &Path, name: &str, state: Before<()>, ours: &str, installed: &Option<String>) -> Result<String, Failure> {
    let before = backup::keep_stock(dir, name, &state, installed, write_executable)?;
    write_executable(&dir.join(name), wrapper(ours, name).as_bytes())?;
    Ok(before)
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
    PROGRAMS.migrate(dir)?;
    let left_out = PROGRAMS.left_out(dir, &names)?;
    let lib = dir.join(LIB);
    let ours = canary(DMFAST_SO).ok_or("internal: embedded libdmfast.so has no canary")?;
    let installed = installed_canary(&lib)?;
    // A wrapped program's backup only has to open: the wrapper runs it as it is.
    let open = |b: &Path| fs::File::open(b).map(drop).map_err(|e| io_err("cannot read", b, e));
    let states = names.iter().map(|n| PROGRAMS.check(dir, n, open)).collect::<Result<Vec<_>, _>>()?;
    // The library first: a wrapper must never point at a library that isn't there.
    write(&lib, DMFAST_SO)?;
    let mut before = None;
    for (name, state) in names.iter().zip(states) {
        let b = wrap(dir, name, state, &ours, &installed)?;
        before.get_or_insert(b);
    }
    for name in &left_out {
        PROGRAMS.restore(dir, name)?;
    }
    drop_empty_backup_dir(dir)?;
    Ok(PROGRAMS.installed_message(dir, &ours, before, &PROGRAMS.parts(&names), &left_out))
}

/// None if dmfast isn't installed in `dir` (nothing of ours was found).
pub fn uninstall(dir: &Path) -> Result<Option<String>, Failure> {
    let lib = dir.join(LIB);
    // Only for the message: an unreadable library must not block removing it.
    let ver = installed_canary(&lib).ok().flatten();
    PROGRAMS.migrate(dir)?;
    let (restored, lost) = PROGRAMS.restore_all(dir)?;
    if restored.is_empty() && lost.is_empty() && !lib.is_file() {
        return Ok(None);
    }
    if lib.is_file() {
        fs::remove_file(&lib).map_err(|e| io_err("cannot remove", &lib, e))?;
    }
    drop_empty_backup_dir(dir)?;
    Ok(Some(PROGRAMS.removed_message(dir, ver, &restored, &lost)))
}
