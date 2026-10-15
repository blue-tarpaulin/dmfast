//! Reading and safely replacing files in a BYOND bin folder, with errors that say what to do.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

pub enum Failure {
    /// Needs admin/root rights (e.g. Program Files, /usr/local): retry elevated.
    Denied(String),
    Other(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Failure::Denied(m) | Failure::Other(m) => m,
        }
    }
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Other(s)
    }
}

impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure::Other(s.to_string())
    }
}

pub fn io_err(what: &str, path: &Path, e: io::Error) -> Failure {
    let msg = format!("{what} {}: {e}", path.display());
    match e.raw_os_error() {
        // Windows sharing violation / lock violation: the file is loaded by a running BYOND program.
        Some(32 | 33) if cfg!(windows) => {
            Failure::Other(format!("{msg}\n  -> BYOND is still running. Close it (pager too) and try again."))
        }
        _ if e.kind() == ErrorKind::PermissionDenied => Failure::Denied(msg),
        _ => Failure::Other(msg),
    }
}

pub fn read(p: &Path) -> Result<Vec<u8>, Failure> {
    fs::read(p).map_err(|e| io_err("cannot read", p, e))
}

/// Rename, replacing `to`. On Windows a file that was just loaded or written is often briefly held open
/// by antivirus or the indexer (access denied / sharing violation), so retry for a couple of seconds.
pub fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if cfg!(windows) && tries < 20 && matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// Write via a temp file + rename so a failure never leaves a half-written file.
pub fn write(p: &Path, data: &[u8]) -> Result<(), Failure> {
    write_mode(p, data, false)
}

/// As [`write`], for a program or script (mode 755 on Unix).
#[cfg(unix)]
pub fn write_executable(p: &Path, data: &[u8]) -> Result<(), Failure> {
    write_mode(p, data, true)
}

fn write_mode(p: &Path, data: &[u8], executable: bool) -> Result<(), Failure> {
    let tmp = p.with_extension("dmfast-tmp");
    fs::write(&tmp, data).map_err(|e| io_err("cannot write", &tmp, e))?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| io_err("cannot chmod", &tmp, e))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    rename_retry(&tmp, p).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_err("cannot replace", p, e)
    })
}
