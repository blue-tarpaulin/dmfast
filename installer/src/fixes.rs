//! `dmfast.ini`, dmfast's settings file next to its library. `--fixes` sets its `fixes` key: which
//! opt-in BYOND bug fixes dmfast applies (BUGFIXES.md lists the fixes and tiers). dmfast reads the file
//! when a BYOND program starts; `DMFAST_FIXES` overrides the key for one run. `--client` keeps its choice of
//! DreamSeeker patches in the `client` key, for the next install.

use std::fs;
use std::path::Path;

use dmfast_format::selection::{CONFIG_FILE, EXPERIMENTAL, Tier, ini_sets, ini_value};

use crate::files::{Failure, io_err, read, write};

const NEW_FILE: &str = "[dmfast]\n";

const FIXES_COMMENT: &str =
    "; bug fixes: safe, extended, breaking, none, or keys (BUGFIXES.md); DMFAST_FIXES overrides it";

/// The entries of a selection (`--fixes` or `--client`).
pub(crate) fn split(spec: &str) -> Vec<String> {
    dmfast_format::selection::split(spec).map(str::to_owned).collect()
}

/// The entries of a `--fixes` value: tier names, `experimental`, fix keys, or `-key` to leave one out.
/// `none` alone means no fixes (no entries).
fn entries(spec: &str) -> Result<Vec<String>, Failure> {
    let entries = split(spec);
    if entries.is_empty() {
        return Err(
            "--fixes needs a value: safe, extended, breaking, experimental, none, or fix keys (see BUGFIXES.md)".into(),
        );
    }
    if entries == ["none"] {
        return Ok(Vec::new());
    }
    for e in &entries {
        let key = e.strip_prefix('-').unwrap_or(e);
        let word = !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let tier_word = Tier::parse(key).is_some() || key == EXPERIMENTAL;
        if !word || key == "none" || (e.starts_with('-') && tier_word) {
            return Err(format!(
                "--fixes: {e:?} is not a tier (safe, extended, breaking), experimental, a fix key, or -key"
            )
            .into());
        }
    }
    Ok(entries)
}

/// Checks a `--fixes` value before anything is installed.
pub fn check(spec: &str) -> Result<(), Failure> {
    entries(spec).map(|_| ())
}

/// Sets the `fixes` key. Returns how the selection now reads.
pub fn set(dir: &Path, spec: &str) -> Result<String, Failure> {
    let entries = entries(spec)?;
    let value = if entries.is_empty() { "none".to_string() } else { entries.join(", ") };
    put(dir, "fixes", &value, FIXES_COMMENT)?;
    Ok(value)
}

/// Sets `key` to `value`, keeping the rest of the file. A new key goes last, after `comment`.
pub(crate) fn put(dir: &Path, key: &str, value: &str, comment: &str) -> Result<(), Failure> {
    let path = dir.join(CONFIG_FILE);
    let old = if path.is_file() { String::from_utf8_lossy(&read(&path)?).into_owned() } else { NEW_FILE.to_string() };
    let line = format!("{key} = {value}");
    let mut lines: Vec<&str> = Vec::new();
    let mut done = false;
    for l in old.lines() {
        if !ini_sets(l, key) {
            lines.push(l);
        } else if !done {
            lines.push(&line);
            done = true;
        }
    }
    if !done {
        lines.extend(comment.lines());
        lines.push(&line);
    }
    write(&path, (lines.join("\n") + "\n").as_bytes())
}

/// The saved selection (the `fixes` value), if there is a settings file with one.
pub fn current(dir: &Path) -> Result<Option<String>, Failure> {
    get(dir, "fixes")
}

/// `key`'s value, if there is a settings file with one.
pub(crate) fn get(dir: &Path, key: &str) -> Result<Option<String>, Failure> {
    let path = dir.join(CONFIG_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&read(&path)?).into_owned();
    Ok(ini_value(&text, key).filter(|v| !v.is_empty()).map(str::to_owned))
}

/// The saved selection, `none` without one.
pub fn describe(dir: &Path) -> Result<String, Failure> {
    Ok(current(dir)?.unwrap_or_else(|| "none".into()))
}

/// Removes the settings file (uninstall).
pub fn remove(dir: &Path) -> Result<(), Failure> {
    let path = dir.join(CONFIG_FILE);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| io_err("cannot remove", &path, e))?;
    }
    Ok(())
}
