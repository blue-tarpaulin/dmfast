//! What the installer prints for `--help`, `--help bugfixes` and `--version`.

/// First line of the help and of the guided install.
macro_rules! banner {
    () => {
        concat!("dmfast ", env!("CARGO_PKG_VERSION"), ": Robusts BYOND's toolchain - Greytide Worldwide 2026")
    };
}

pub const BANNER: &str = banner!();

pub const VERSION: &str = concat!(
    "dmfast ",
    env!("CARGO_PKG_VERSION"),
    " - Greytide Worldwide 2026\n\nBundles mimalloc (https://github.com/microsoft/mimalloc),\n\
     MIT License, Copyright (c) 2018-2026 Microsoft Corporation, Daan Leijen."
);

#[cfg(windows)]
pub const HELP: &str = concat!(
    banner!(),
    r#"

dmfast-install [options] <BYOND or bin folder, dm.exe or dreamdaemon.exe>
  options:
     --uninstall        remove dmfast and restores originals
     --no-runtime       leave DreamDaemon stock (only speed up the compiler)
     --fixes <which>    opt-in bug fixes: 'safe', 'extended' (recommended), 'breaking',
                        or fix keys that are comma-separated; -key leaves one out
     --version          show version and licenses
     -h, --help         view this help
     --help bugfixes    view a list of the possible bugfixes

  examples:
     dmfast-install "C:\Program Files (x86)\BYOND\bin"
     dmfast-install --fixes safe "C:\Program Files (x86)\BYOND"
     dmfast-install --uninstall "C:\BYOND"

Close BYOND first. Double-click this exe for a guided install, or drag BYOND onto it.
Re-run this installer after a BYOND update."#
);

#[cfg(not(windows))]
pub const HELP: &str = concat!(
    banner!(),
    r#"

dmfast-install [options] <BYOND or bin folder, DreamMaker or DreamDaemon>
  options:
     --uninstall        remove dmfast and restores originals
     --no-runtime       leave DreamDaemon stock (only speed up the compiler)
     --fixes <which>    opt-in bug fixes: 'safe', 'extended' (recommended), 'breaking',
                        or fix keys that are comma-separated; -key leaves one out
     --version          show version and licenses
     -h, --help         view this help
     --help bugfixes    view a list of the possible bugfixes

  examples:
     dmfast-install ~/byond/bin
     sudo dmfast-install --fixes safe /usr/local/byond
     dmfast-install --uninstall ~/byond

Close BYOND first. Run it with no arguments in a terminal for a guided install.
Re-run this installer after a BYOND update."#
);

/// Generated from BUGFIXES.md's tables by build.rs.
pub const BUGFIXES: &str = concat!(
    "dmfast bug fixes (details in BUGFIXES.md). 'safe' and 'extended' are recommended;\n\
     'breaking' also turns on fixes that cause runtime errors in code that relied on the bugs.\n",
    include_str!(concat!(env!("OUT_DIR"), "/bugfixes.txt"))
);
