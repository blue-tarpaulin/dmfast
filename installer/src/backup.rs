//! The stock copies of what dmfast changes, kept in `bin/backup/`, and the install and uninstall steps both OSes share
//! for the programs dmfast goes into.

use std::fs;
use std::path::{Path, PathBuf};

use crate::files::{Failure, io_err, read, rename_retry};

/// The folder in `bin` holding the stock copy of every file dmfast changed (and nothing else).
pub(crate) const BACKUP: &str = "backup";

pub(crate) fn backup_of(dir: &Path, file: &str) -> PathBuf {
    dir.join(BACKUP).join(file)
}

pub(crate) fn make_backup_dir(dir: &Path) -> Result<(), Failure> {
    let folder = dir.join(BACKUP);
    fs::create_dir_all(&folder).map_err(|e| io_err("cannot create", &folder, e))
}

/// Removes `backup/` once nothing is left in it.
pub(crate) fn drop_empty_backup_dir(dir: &Path) -> Result<(), Failure> {
    let folder = dir.join(BACKUP);
    if fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none()) {
        fs::remove_dir(&folder).map_err(|e| io_err("cannot remove", &folder, e))?;
    }
    Ok(())
}

/// The programs dmfast goes into on one OS, and how to tell one has it.
pub(crate) struct Programs {
    /// Every program dmfast may go into, the compiler first.
    pub(crate) all: &'static [&'static str],
    /// Where earlier builds kept a program's stock copy; [`Programs::migrate`] moves it to `backup/`.
    pub(crate) old_backup_of: fn(&Path, &str) -> PathBuf,
    /// Whether a program's bytes have dmfast.
    pub(crate) has_dmfast: fn(&[u8]) -> Result<bool, Failure>,
    /// How messages say a program has dmfast (`has dmfast`).
    pub(crate) has: &'static str,
    /// How messages name a program's backup.
    pub(crate) shown_backup: fn(&Path, &str) -> String,
}

/// What a program is before installing: stock (its bytes become the backup), or already has dmfast, with what
/// [`Programs::check`] kept of its backup (also when the program itself was deleted).
pub(crate) enum Before<T> {
    Stock(Vec<u8>),
    Ours(T),
}

/// What [`Programs::restore`] would do to a program.
pub(crate) enum Restore {
    /// Has dmfast: its backup goes back.
    Backup,
    /// Stock: left alone.
    NotOurs,
    /// Has dmfast, but the backup to restore is gone.
    NoBackup,
}

impl Programs {
    pub(crate) fn compiler(&self) -> &'static str {
        self.all[0]
    }

    pub(crate) fn has_stock_copy(&self, dir: &Path, exe: &str) -> bool {
        backup_of(dir, exe).is_file() || (self.old_backup_of)(dir, exe).is_file()
    }

    /// Moves each program's stock copy from where earlier builds kept it into `backup/`.
    pub(crate) fn migrate(&self, dir: &Path) -> Result<(), Failure> {
        for name in self.all {
            let old = (self.old_backup_of)(dir, name);
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

    /// Whether `dir` holds the compiler: the program, or only its stock copy when the one with dmfast was deleted.
    pub(crate) fn has_compiler(&self, dir: &Path) -> bool {
        dir.join(self.compiler()).is_file() || self.has_stock_copy(dir, self.compiler())
    }

    /// The folder `target` names: itself, the folder of a file, or the `bin` in the BYOND folder. The caller checks
    /// it holds BYOND.
    pub(crate) fn bin_dir(&self, target: &Path) -> PathBuf {
        let dir = if target.is_file() { target.parent().unwrap_or(Path::new(".")) } else { target };
        let bin = dir.join("bin");
        if !self.has_compiler(dir) && self.has_compiler(&bin) { bin } else { dir.to_path_buf() }
    }

    pub(crate) fn missing_backup(&self, dir: &Path, name: &str) -> Failure {
        format!("{name} {} but {} is missing; reinstall BYOND", self.has, (self.shown_backup)(dir, name)).into()
    }

    /// Reads everything installing `name` needs (`keep` checks or reads its backup), so a program that can't be read
    /// stops the install before anything changes.
    pub(crate) fn check<T>(
        &self,
        dir: &Path,
        name: &str,
        keep: impl FnOnce(&Path) -> Result<T, Failure>,
    ) -> Result<Before<T>, Failure> {
        let exe = dir.join(name);
        if exe.is_file() {
            let data = read(&exe)?;
            if !(self.has_dmfast)(&data)? {
                return Ok(Before::Stock(data));
            }
        }
        let backup = backup_of(dir, name);
        if !backup.is_file() {
            return Err(self.missing_backup(dir, name));
        }
        Ok(Before::Ours(keep(&backup)?))
    }

    /// Whether `name` has dmfast (or only its backup is left) and its backup is there. Reads only.
    pub(crate) fn restore_check(&self, dir: &Path, name: &str) -> Result<Restore, Failure> {
        let exe = dir.join(name);
        if exe.is_file() && !(self.has_dmfast)(&read(&exe)?)? {
            return Ok(Restore::NotOurs);
        }
        if !backup_of(dir, name).is_file() {
            return Ok(if exe.is_file() { Restore::NoBackup } else { Restore::NotOurs });
        }
        Ok(Restore::Backup)
    }

    /// Puts `name`'s stock copy back from its backup.
    pub(crate) fn restore(&self, dir: &Path, name: &str) -> Result<(), Failure> {
        let exe = dir.join(name);
        rename_retry(&backup_of(dir, name), &exe).map_err(|e| io_err("cannot restore", &exe, e))
    }

    /// The programs not in `names` that have dmfast: install puts them back (re-running without one means "not this
    /// one"). Fails if one's backup is gone.
    pub(crate) fn left_out(&self, dir: &Path, names: &[&str]) -> Result<Vec<&'static str>, Failure> {
        let mut left_out = Vec::new();
        for &name in self.all.iter().filter(|n| !names.contains(n)) {
            match self.restore_check(dir, name)? {
                Restore::Backup => left_out.push(name),
                Restore::NotOurs => {}
                Restore::NoBackup => return Err(self.missing_backup(dir, name)),
            }
        }
        Ok(left_out)
    }

    /// What the programs are, for messages: `compiler`, `server`.
    pub(crate) fn parts(&self, names: &[&str]) -> Vec<&'static str> {
        let mut parts = Vec::new();
        if names.contains(&self.compiler()) {
            parts.push("compiler");
        }
        if names.iter().any(|n| self.all[1..].contains(n)) {
            parts.push("server");
        }
        parts
    }

    /// The install message: `ours` installed in `dir` over what the first program had `before`, for `parts`, and the
    /// programs `left_out` put back.
    pub(crate) fn installed_message(
        &self,
        dir: &Path,
        ours: &str,
        before: Option<String>,
        parts: &[&str],
        left_out: &[&str],
    ) -> String {
        let mut msg = match before {
            Some(b) if b == ours => format!("reinstalled {ours} in {}", dir.display()),
            Some(b) => format!("installed {ours} in {} (was {b})", dir.display()),
            None => format!("installed {ours} in {}", dir.display()),
        };
        msg += &format!(" for {}", parts.join(", "));
        if !left_out.is_empty() {
            msg += &format!("; restored stock {}", self.parts(left_out).join(", "));
        }
        msg
    }

    /// Puts every program's backup back. A program whose backup is gone is removed: without dmfast it could never start
    /// again, and removing it lets uninstall finish. Returns the programs restored and the ones removed.
    pub(crate) fn restore_all(&self, dir: &Path) -> Result<(Vec<&'static str>, Vec<&'static str>), Failure> {
        let (mut restored, mut lost) = (Vec::new(), Vec::new());
        for &name in self.all {
            match self.restore_check(dir, name)? {
                Restore::Backup => {
                    self.restore(dir, name)?;
                    restored.push(name);
                }
                Restore::NotOurs => {}
                Restore::NoBackup => {
                    let exe = dir.join(name);
                    fs::remove_file(&exe).map_err(|e| io_err("cannot remove", &exe, e))?;
                    lost.push(name);
                }
            }
        }
        Ok((restored, lost))
    }

    /// The uninstall message for what [`Programs::restore_all`] did.
    pub(crate) fn removed_message(&self, dir: &Path, ver: Option<String>, restored: &[&str], lost: &[&str]) -> String {
        let mut msg = format!("removed {} from {}", ver.unwrap_or_else(|| "dmfast".into()), dir.display());
        if !restored.is_empty() {
            msg += &format!(" (restored {})", restored.join(", "));
        }
        for name in lost {
            msg += &format!(
                "\nwarning: {} was missing, so {name} is gone; reinstall BYOND to get it back",
                (self.shown_backup)(dir, name)
            );
        }
        msg
    }
}

/// Keeps a stock program's bytes as its backup, written with `write`. Returns what was there before: `stock`, or the
/// dmfast version `installed` (the one installed now).
pub(crate) fn keep_stock<T>(
    dir: &Path,
    name: &str,
    state: &Before<T>,
    installed: &Option<String>,
    write: fn(&Path, &[u8]) -> Result<(), Failure>,
) -> Result<String, Failure> {
    Ok(match state {
        Before::Ours(_) => installed.clone().unwrap_or_else(|| "dmfast (unknown version)".into()),
        // Stock (fresh install, or BYOND updated over us): it becomes the backup.
        Before::Stock(data) => {
            let backup = backup_of(dir, name);
            let before = if backup.is_file() { "stock (BYOND was updated)" } else { "stock" };
            make_backup_dir(dir)?;
            write(&backup, data)?;
            before.to_string()
        }
    })
}
