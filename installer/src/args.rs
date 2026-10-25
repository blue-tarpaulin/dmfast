//! Command-line options.

use std::ffi::OsString;
use std::path::PathBuf;

pub struct Opts {
    pub target: PathBuf,
    pub uninstall: bool,
    /// Also patch DreamDaemon (the default; `--no-runtime` turns it off).
    pub runtime: bool,
    /// `--fixes`: the bug-fix selection to save; None keeps the current one.
    pub fixes: Option<String>,
    /// Windows: this is the relaunched admin copy.
    #[cfg(windows)]
    pub elevated: bool,
}

pub enum Action {
    Help,
    BugFixes,
    Version,
    Run(Opts),
    /// A usage mistake: the message to print (exit status 1).
    Error(String),
}

/// Paths may be any bytes (`OsString`); options are ASCII, so an argument that isn't valid Unicode is a path.
pub fn parse(args: &[OsString]) -> Action {
    let (mut target, mut uninstall, mut runtime, mut fixes) = (None, false, true, None);
    #[cfg(windows)]
    let mut elevated = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let a = arg.to_str().unwrap_or("");
        match a {
            "--version" | "-V" => return Action::Version,
            "--uninstall" => uninstall = true,
            "--no-runtime" => runtime = false,
            // Needed before DreamDaemon was on by default; still accepted so existing scripts keep working.
            "--runtime" => runtime = true,
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
                return if args.next().and_then(|v| v.to_str()) == Some("bugfixes") {
                    Action::BugFixes
                } else {
                    Action::Help
                };
            }
            _ if a.starts_with('-') => return Action::Error(format!("unknown option {a:?} (see --help)")),
            _ if target.is_some() => return Action::Error("more than one BYOND folder given (see --help)".into()),
            _ => target = Some(PathBuf::from(arg)),
        }
    }
    let Some(target) = target else { return Action::Error("no BYOND folder given (see --help)".into()) };
    Action::Run(Opts {
        target,
        uninstall,
        runtime,
        fixes,
        #[cfg(windows)]
        elevated,
    })
}
