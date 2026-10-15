//! Guided install: started with no arguments (double-clicked on Windows, run bare in a terminal on Linux),
//! the installer asks where BYOND is and what to install, then runs as if given the matching options.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::{fixes, platform};

/// One line from stdin, trimmed; None at end of input.
fn line() -> Option<String> {
    let mut s = String::new();
    (io::stdin().lock().read_line(&mut s).ok()? > 0).then(|| s.trim().to_string())
}

fn ask(question: &str) -> Option<String> {
    print!("{question} ");
    io::stdout().flush().ok()?;
    line()
}

/// A yes/no question; Enter takes `default`.
fn yes(question: &str, default: bool) -> Option<bool> {
    loop {
        let a = ask(&format!("{question} [{}]", if default { "Y/n" } else { "y/N" }))?.to_ascii_lowercase();
        match a.as_str() {
            "" => return Some(default),
            "y" | "yes" => return Some(true),
            "n" | "no" => return Some(false),
            _ => {}
        }
    }
}

/// A folder typed, pasted or dropped onto the window: terminals and Explorer quote paths with spaces.
fn folder(prompt: &str) -> Option<PathBuf> {
    loop {
        let a = ask(prompt)?;
        let a = a.trim_matches(['"', '\'']);
        if a.is_empty() {
            return None;
        }
        let p = PathBuf::from(a);
        match platform::bin_dir(&p) {
            Ok(_) => return Some(p),
            Err(e) => println!("{}", e.message()),
        }
    }
}

/// The arguments the answers stand for, or None if the user cancelled.
pub fn run() -> Option<Vec<String>> {
    println!("{}\n", crate::help::BANNER);
    println!("Guided install - for a manual install run it with --help.\n");
    let found = platform::byond_dirs();
    let mut target = None;
    for d in &found {
        if platform::bin_dir(d).is_ok() && yes(&format!("BYOND found in {}. Use it?", d.display()), true)? {
            target = Some(d.clone());
            break;
        }
    }
    let target = match target {
        Some(t) => t,
        None => folder("BYOND folder (drag BYOND here or paste its path; Enter cancels):")?,
    };
    let bin = platform::bin_dir(&target).ok()?;
    let (installed, runtime_now) = platform::state(&bin);
    let path = target.to_string_lossy().into_owned();
    if installed {
        loop {
            match ask("dmfast is installed there. [U]pdate, [r]emove or [c]ancel?")?.to_ascii_lowercase().as_str() {
                "" | "u" | "update" => break,
                "r" | "remove" => return Some(vec!["--uninstall".into(), path]),
                "c" | "cancel" => return None,
                _ => {}
            }
        }
    }
    let mut args = Vec::new();
    if !yes("Speed up DreamDaemon too?", !installed || runtime_now)? {
        args.push("--no-runtime".into());
    }
    let now = fixes::current(&bin).ok().flatten();
    let keep = now.as_deref().unwrap_or("none");
    println!("Bug fixes ('--help bugfixes' lists them):");
    loop {
        let a = ask(&format!("  (n)one, (s)afe or (e)xtended? [{keep}]"))?.to_ascii_lowercase();
        let pick = match a.as_str() {
            "" => break,
            "n" | "none" => "none",
            "s" | "safe" => "safe",
            "e" | "extended" => "extended",
            _ => continue,
        };
        args.extend(["--fixes".into(), pick.into()]);
        break;
    }
    args.push(path);
    println!();
    Some(args)
}
