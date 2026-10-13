//! dmfast-install: installs dmfast into a BYOND `bin` folder.
//!
//! Windows: drag `dm.exe` (or the bin folder) onto this exe; see `windows`. Linux: run it on the bin
//! folder that holds `DreamMaker`; see `linux`. Running it again updates dmfast; after a BYOND update it
//! picks up the new compiler. `--uninstall` puts the original back.

mod files;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

use files::Failure;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
use windows as platform;

const VERSION: &str = concat!(
    "dmfast ",
    env!("CARGO_PKG_VERSION"),
    " - Greytide Worldwide 2026\n\nBundles mimalloc (https://github.com/microsoft/mimalloc),\n\
     MIT License, Copyright (c) 2018-2026 Microsoft Corporation, Daan Leijen."
);

struct Opts {
    target: Option<PathBuf>,
    uninstall: bool,
    /// Also patch DreamDaemon (the default; `--no-runtime` turns it off).
    runtime: bool,
    /// Windows: this is the relaunched admin copy.
    #[cfg(windows)]
    elevated: bool,
}

#[cfg(windows)]
const HELP: &str = concat!(
    "dmfast ",
    env!("CARGO_PKG_VERSION"),
    r#": Robusts BYOND's toolchain - Greytide Worldwide 2026

dmfast-install [options] <BYOND bin folder or dm.exe>
  options:
     --uninstall        remove dmfast (restores the original programs)
     --no-runtime       leave DreamDaemon stock (only speed up the compiler)
     --version          show version and licenses
     -h, --help         view this help

  examples:
     dmfast-install "C:\Program Files (x86)\BYOND\bin"
     dmfast-install --no-runtime "C:\Program Files (x86)\BYOND\bin"
     dmfast-install --uninstall "C:\Program Files (x86)\BYOND\bin"

Close BYOND first. You can also drag dm.exe or the bin folder onto dmfast-install.exe.
Re-run the install after a BYOND update. Installing with --no-runtime puts back stock DreamDaemon."#
);
#[cfg(not(windows))]
const HELP: &str = concat!(
    "dmfast ",
    env!("CARGO_PKG_VERSION"),
    r#": Robusts BYOND's toolchain - Greytide Worldwide 2026

dmfast-install [options] <BYOND bin folder or DreamMaker>
  options:
     --uninstall        remove dmfast (restores the original programs)
     --no-runtime       leave DreamDaemon stock (only speed up the compiler)
     --version          show version and licenses
     -h, --help         view this help

  examples:
     dmfast-install ~/byond/bin
     sudo dmfast-install /usr/local/byond/bin
     dmfast-install --uninstall ~/byond/bin

DreamMaker and DreamDaemon become small scripts that preload dmfast. Re-run the install after a
BYOND update. Installing with --no-runtime puts back stock DreamDaemon."#
);

/// None: show help.
fn parse_args() -> Option<Opts> {
    let mut o = Opts {
        target: None,
        uninstall: false,
        runtime: true,
        #[cfg(windows)]
        elevated: false,
    };
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--uninstall" => o.uninstall = true,
            "--no-runtime" => o.runtime = false,
            // Needed before DreamDaemon was on by default; still accepted so existing scripts keep working.
            "--runtime" => o.runtime = true,
            #[cfg(windows)]
            "--elevated" => o.elevated = true,
            "-h" | "--help" | "/?" => return None,
            _ => o.target = Some(PathBuf::from(a)),
        }
    }
    o.target.is_some().then_some(o)
}

/// Keep a double-clicked / drag-dropped console window open until the result is read.
fn pause() {
    #[cfg(windows)]
    platform::pause_if_own_console();
}

fn main() {
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("{VERSION}");
        return;
    }
    let Some(o) = parse_args() else {
        println!("{HELP}");
        pause();
        return;
    };
    let target = o.target.as_deref().unwrap();
    let result = platform::bin_dir(target)
        .and_then(|d| if o.uninstall { platform::uninstall(&d) } else { platform::install(&d, o.runtime) });
    let code = match result {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        #[cfg(windows)]
        Err(Failure::Denied(msg)) if !o.elevated => {
            println!("{msg}\nasking for admin rights...");
            if platform::relaunch_elevated() {
                return; // the elevated copy reports (and pauses) itself
            }
            println!("could not get admin rights");
            1
        }
        #[cfg(not(windows))]
        Err(Failure::Denied(msg)) => {
            println!("error: {msg}\n  -> run it with sudo");
            1
        }
        #[cfg(windows)]
        Err(Failure::Denied(msg)) => {
            println!("error: {msg}");
            1
        }
        Err(Failure::Other(msg)) => {
            println!("error: {msg}");
            1
        }
    };
    pause();
    std::process::exit(code);
}
