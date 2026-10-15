//! `dmfast.ini`, dmfast's settings file next to its library. `--fixes` sets its `fixes` key: which
//! opt-in BYOND bug fixes dmfast applies (BUGFIXES.md lists the fixes and tiers). dmfast reads the file
//! when a BYOND program starts; `DMFAST_FIXES` overrides the key for one run.

use std::fs;
use std::path::Path;

use crate::files::{Failure, io_err, read, write};

/// Read by dmfast (`CONFIG_FILE` in the patches crate); keep the names in step.
const FILE: &str = "dmfast.ini";

const TIERS: [&str; 3] = ["safe", "extended", "breaking"];

const NEW_FILE: &str = "; dmfast settings, read when a BYOND program starts. dmfast-install keeps edits to this file.\n\
                        [dmfast]\n\
                        ; Opt-in BYOND bug fixes (see BUGFIXES.md): safe or extended (recommended), breaking (causes\n\
                        ; runtime errors in code that relied on the bugs), none, or fix keys, comma-separated; -key\n\
                        ; leaves one out. DMFAST_FIXES, if set, overrides this for a run.\n";

/// The entries of a `--fixes` value: tier names, fix keys, or `-key` to leave one out, separated by commas.
/// `none` alone means no fixes (no entries).
fn entries(spec: &str) -> Result<Vec<String>, Failure> {
    let entries: Vec<String> =
        spec.split(|c: char| c == ',' || c.is_whitespace()).filter(|e| !e.is_empty()).map(str::to_owned).collect();
    if entries.is_empty() {
        return Err("--fixes needs a value: safe, extended, breaking, none, or fix keys (see BUGFIXES.md)".into());
    }
    if entries == ["none"] {
        return Ok(Vec::new());
    }
    for e in &entries {
        let key = e.strip_prefix('-').unwrap_or(e);
        let word = !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !word || key == "none" || (e.starts_with('-') && TIERS.contains(&key)) {
            return Err(format!("--fixes: {e:?} is not a tier (safe, extended, breaking), a fix key, or -key").into());
        }
    }
    Ok(entries)
}

/// Whether an INI line sets `fixes` (comments and surrounding spaces ignored).
fn is_fixes_line(line: &str) -> bool {
    line.split([';', '#']).next().unwrap_or("").split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("fixes"))
}

/// Checks a `--fixes` value before anything is installed.
pub fn check(spec: &str) -> Result<(), Failure> {
    entries(spec).map(|_| ())
}

/// Sets the `fixes` key, keeping the rest of the file. Returns how the selection now reads.
pub fn set(dir: &Path, spec: &str) -> Result<String, Failure> {
    let entries = entries(spec)?;
    let value = if entries.is_empty() { "none".to_string() } else { entries.join(", ") };
    let path = dir.join(FILE);
    let old = if path.is_file() { String::from_utf8_lossy(&read(&path)?).into_owned() } else { NEW_FILE.to_string() };
    let line = format!("fixes = {value}");
    let mut lines: Vec<String> = Vec::new();
    let mut done = false;
    for l in old.lines() {
        if is_fixes_line(l) {
            if !done {
                lines.push(line.clone());
                done = true;
            }
        } else {
            lines.push(l.to_string());
        }
    }
    if !done {
        lines.push(line);
    }
    write(&path, (lines.join("\n") + "\n").as_bytes())?;
    describe(dir)
}

/// The saved selection (the `fixes` value), if there is a settings file with one.
pub fn current(dir: &Path) -> Result<Option<String>, Failure> {
    let path = dir.join(FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&read(&path)?).into_owned();
    Ok(text
        .lines()
        .rev()
        .filter(|l| is_fixes_line(l))
        .find_map(|l| l.split([';', '#']).next()?.split_once('=').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty()))
}

/// How the saved selection reads, for the install message.
pub fn describe(dir: &Path) -> Result<String, Failure> {
    Ok(format!("bug fixes: {}", current(dir)?.unwrap_or_else(|| "none".into())))
}

/// Removes the settings file (uninstall).
pub fn remove(dir: &Path) -> Result<(), Failure> {
    let path = dir.join(FILE);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| io_err("cannot remove", &path, e))?;
    }
    Ok(())
}
