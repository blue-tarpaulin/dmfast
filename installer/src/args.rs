//! Command-line options.

use std::ffi::OsString;
use std::path::PathBuf;

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
    /// `--client`: also patch DreamSeeker, with the DreamSeeker patches the saved fix selection picks (`safe` without
    /// one).
    #[cfg(windows)]
    pub client: bool,
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
    let (mut audit, mut client, mut elevated) = (false, false, false);
    let mut args = args.iter();
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
            "--client" => client = true,
            "--fixes" => match args.next().and_then(|v| v.to_str()) {
                Some(v) if !v.is_empty() && !v.starts_with("--") => fixes = Some(v.to_string()),
                _ => return Action::Error("--fixes needs a value: safe, extended, breaking, none, or fix keys".into()),
            },
            _ if a.starts_with("--fixes=") => match &a["--fixes=".len()..] {
                "" => {
                    return Action::Error("--fixes needs a value: safe, extended, breaking, none, or fix keys".into());
                }
                v => fixes = Some(v.to_string()),
            },
            #[cfg(windows)]
            "--elevated" => elevated = true,
            "-h" | "--help" | "/?" => {
                return match args.next().and_then(|v| v.to_str()) {
                    Some("bugfixes") => Action::BugFixes,
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
    let options = options || client;
    #[cfg(windows)]
    if audit && (uninstall || options) {
        return Action::Error("--audit only reads; it takes no other option".into());
    }
    if uninstall && options {
        return Action::Error("--uninstall takes no other option".into());
    }
    #[cfg(windows)]
    if !compiler && !server && !client {
        return Action::Error(
            "--no-compiler and --no-server leave nothing to install (add --client for DreamSeeker)".into(),
        );
    }
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
