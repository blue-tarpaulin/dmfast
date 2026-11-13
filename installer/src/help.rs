//! What the installer prints for `--help`, `--help fixes` and `--version`.

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
     --uninstall        remove dmfast and restore stock
     --audit            check the installed DreamSeeker patches
     --no-compiler      leave the compiler stock
     --no-server        leave DreamDaemon stock
     --fixes [which]    bug fixes: safe (default), extended, breaking, none, or keys;
                        -key leaves one out.
     --client [which]   DreamSeeker patches, picked like --fixes
     --version          show version and licenses
     -h, --help         this help
     --help fixes       list the bug fixes
     --help client      list the DreamSeeker patches

  examples:
     dmfast-install "C:\Program Files (x86)\BYOND"
     dmfast-install --fixes extended --client "C:\Program Files (x86)\BYOND"
     dmfast-install --uninstall "C:\Program Files (x86)\BYOND"

Close BYOND first. Double-click this exe for a guided install, or drag BYOND onto it.
Re-run this installer after a BYOND update."#
);

#[cfg(not(windows))]
pub const HELP: &str = concat!(
    banner!(),
    r#"

dmfast-install [options] <BYOND or bin folder, DreamMaker or DreamDaemon>
  options:
     --uninstall        remove dmfast and restore stock
     --no-compiler      leave the compiler stock
     --no-server        leave DreamDaemon stock
     --fixes [which]    bug fixes: safe (default), extended, breaking, none, or keys;
                        -key leaves one out.
     --version          show version and licenses
     -h, --help         this help
     --help fixes       list the bug fixes

  examples:
     dmfast-install ~/byond/bin
     sudo dmfast-install --fixes extended /usr/local/byond
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
