//! Which edition a build script builds: full (with the `patches-private` submodule) or open-source (`-oss`).
//! `DMFAST_EDITION=oss` leaves the private patches out even when the submodule is checked out, `full` requires them,
//! and unset uses them when the submodule is checked out.

use std::path::Path;

/// Whether this build includes the private patches. `root` is the workspace root. Panics on a `DMFAST_EDITION` that
/// isn't `oss` or `full`, and on `full` without the submodule. Tells cargo to rerun the build script on either changing.
pub fn private(root: &Path) -> bool {
    let lib = root.join("patches-private/src/lib.rs");
    println!("cargo:rerun-if-env-changed=DMFAST_EDITION");
    println!("cargo:rerun-if-changed={}", lib.display());
    match std::env::var("DMFAST_EDITION").as_deref() {
        Ok("oss") => false,
        Ok("full") => {
            assert!(
                lib.exists(),
                "DMFAST_EDITION=full needs patches-private: git submodule update --init patches-private"
            );
            true
        }
        Ok("") | Err(_) => lib.exists(),
        Ok(other) => panic!("DMFAST_EDITION={other}: use oss or full"),
    }
}

/// The version this build reports: the package's, with `-oss` when it leaves the private patches out.
pub fn version(private: bool) -> String {
    format!("{}{}", env!("CARGO_PKG_VERSION"), if private { "" } else { "-oss" })
}
