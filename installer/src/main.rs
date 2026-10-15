//! dmfast-install: installs dmfast into a BYOND `bin` folder.
//!
//! Windows: double-click for a guided install, or drag `dm.exe` (or the bin folder) onto this exe; see
//! `windows`. Linux: run it on the BYOND or bin folder, or with no arguments in a terminal for a guided
//! install; see `linux`. Running it again updates dmfast; after a BYOND update it picks up the new
//! compiler. `--uninstall` puts the original back.

mod args;
mod files;
mod fixes;
mod guided;
mod help;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use args::{Action, Opts};
use files::Failure;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
use windows as platform;

use std::ffi::OsString;

/// Keep a double-clicked / drag-dropped console window open until the result is read.
fn pause() {
    #[cfg(windows)]
    platform::pause_if_own_console();
}

/// No arguments and someone at the keyboard: a double-clicked console on Windows, a terminal on Linux.
fn interactive() -> bool {
    use std::io::IsTerminal;
    #[cfg(windows)]
    let launched_alone = platform::own_console();
    #[cfg(not(windows))]
    let launched_alone = true;
    launched_alone && std::io::stdin().is_terminal()
}

/// Installs or uninstalls as `o` says; the message to print.
fn run(o: &Opts) -> Result<String, Failure> {
    if let Some(spec) = &o.fixes {
        fixes::check(spec)?;
    }
    let d = platform::bin_dir(&o.target)?;
    if o.uninstall {
        // Only remove the settings file when dmfast was actually there: it may be the user's own.
        return Ok(match platform::uninstall(&d)? {
            Some(msg) => {
                fixes::remove(&d)?;
                msg
            }
            None => format!("dmfast is not installed in {}", d.display()),
        });
    }
    // Settings first: on its own the file changes nothing, and a failure here leaves BYOND untouched.
    let fixes = match &o.fixes {
        Some(spec) => fixes::set(&d, spec)?,
        None => fixes::describe(&d)?,
    };
    let msg = platform::install(&d, o.runtime)?;
    Ok(format!("{msg}\n{fixes}"))
}

fn main() {
    let mut argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    if argv.is_empty() {
        if !interactive() {
            println!("{}", help::HELP);
            pause();
            return;
        }
        match guided::run() {
            Some(a) => argv = a.into_iter().map(OsString::from).collect(),
            None => {
                println!("cancelled");
                pause();
                return;
            }
        }
    }
    let o = match args::parse(&argv) {
        Action::Run(o) => o,
        Action::Version => {
            println!("{}", help::VERSION);
            return;
        }
        Action::Help => {
            println!("{}", help::HELP);
            pause();
            return;
        }
        Action::BugFixes => {
            print!("{}", help::BUGFIXES);
            pause();
            return;
        }
        Action::Error(msg) => {
            eprintln!("error: {msg}");
            pause();
            std::process::exit(1);
        }
    };
    let code = match run(&o) {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        #[cfg(windows)]
        Err(Failure::Denied(msg)) if !o.elevated => {
            println!("{msg}\nasking for admin rights...");
            // The guided answers are in `argv`, so the admin copy doesn't ask again.
            let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            if platform::relaunch_elevated(&argv) {
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
