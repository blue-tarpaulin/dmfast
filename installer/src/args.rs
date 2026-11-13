//! Command-line options.

use std::ffi::OsString;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::slice::Iter;

use crate::fixes;

pub struct Opts {
    pub target: PathBuf,
    pub uninstall: bool,
    /// `--audit`: check DreamSeeker's patches in the DLLs.
    #[cfg(windows)]
    pub audit: bool,
    /// Speed up the compiler (the default; `--no-compiler` leaves it stock).
    pub compiler: bool,
    /// Speed up DreamDaemon (the default; `--no-server` leaves it stock).
    pub server: bool,
    /// `--client`: the DreamSeeker patch selection to save; None keeps the current one.
    #[cfg(windows)]
    pub client: Option<String>,
    /// `--fixes`: the bug-fix selection to save; None keeps the current one.
    pub fixes: Option<String>,
    /// Windows: this is the relaunched admin copy.
    #[cfg(windows)]
    pub elevated: bool,
}

pub enum Action {
    Help,
    BugFixes,
    #[cfg(windows)]
    ClientPatches,
    Version,
    Run(Opts),
    /// A usage mistake: the message to print (exit status 1).
    Error(String),
}

/// Paths may be any bytes (`OsString`); options are ASCII, so an argument that isn't valid Unicode is a path.
pub fn parse(args: &[OsString]) -> Action {
    let (mut target, mut uninstall, mut compiler, mut server, mut fixes) = (None, false, true, true, None);
    #[cfg(windows)]
    let (mut audit, mut client, mut elevated) = (false, None, false);
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        let a = arg.to_str().unwrap_or("");
        match a {
            "--version" | "-V" => return Action::Version,
            "--uninstall" => uninstall = true,
            #[cfg(windows)]
            "--audit" => audit = true,
            "--no-compiler" => compiler = false,
            "--no-server" => server = false,
            #[cfg(windows)]
            "--client" => client = Some(value(&mut args, |v| crate::platform::client_selection(v).is_ok())),
            #[cfg(windows)]
            _ if a.starts_with("--client=") => client = Some(a["--client=".len()..].to_string()),
            "--fixes" => fixes = Some(value(&mut args, |v| fixes::check(v).is_ok())),
            _ if a.starts_with("--fixes=") => fixes = Some(a["--fixes=".len()..].to_string()),
            #[cfg(windows)]
            "--elevated" => elevated = true,
            "-h" | "--help" | "/?" => {
                return match args.next().and_then(|v| v.to_str()) {
                    Some("fixes") => Action::BugFixes,
                    #[cfg(windows)]
                    Some("client") => Action::ClientPatches,
                    _ => Action::Help,
                };
            }
            _ if a.starts_with('-') => return Action::Error(format!("unknown option {a:?} (see --help)")),
            _ if target.is_some() => return Action::Error("more than one BYOND folder given (see --help)".into()),
            _ => target = Some(PathBuf::from(arg)),
        }
    }
    let Some(target) = target else { return Action::Error("no BYOND folder given (see --help)".into()) };
    // Options that change what an install does.
    let options = !compiler || !server || fixes.is_some();
    #[cfg(windows)]
    let options = options || client.is_some();
    #[cfg(windows)]
    if audit && (uninstall || options) {
        return Action::Error("--audit only reads; it takes no other option".into());
    }
    if uninstall && options {
        return Action::Error("--uninstall takes no other option".into());
    }
    // Windows: the install says so, since a saved --client may still patch DreamSeeker.
    #[cfg(not(windows))]
    if !compiler && !server {
        return Action::Error("--no-compiler and --no-server leave nothing to install".into());
    }
    Action::Run(Opts {
        target,
        uninstall,
        #[cfg(windows)]
        audit,
        compiler,
        server,
        #[cfg(windows)]
        client,
        fixes,
        #[cfg(windows)]
        elevated,
    })
}

/// The value of `--fixes` or `--client`: the next argument if `valid` takes it and it isn't the BYOND folder, else
/// `safe`.
fn value(args: &mut Peekable<Iter<OsString>>, valid: impl Fn(&str) -> bool) -> String {
    let next = args.next_if(|v| v.to_str().is_some_and(|v| valid(v) && !Path::new(v).exists()));
    next.map_or_else(|| "safe".into(), |v| v.to_string_lossy().into_owned())
}
