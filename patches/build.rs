//! The version, `<version>-oss` in the open-source build (`DMFAST_EDITION`, as `dmfast/build.rs` decides it). Windows:
//! embeds an `asInvoker` manifest in the test executable; without one, Windows asks for elevation to run an .exe named
//! with 'patch'.

fn main() {
    let private = dmfast_format::edition::private(std::path::Path::new(".."));
    println!("cargo:rustc-env=DMFAST_VERSION={}", dmfast_format::edition::version(private));
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='asInvoker'");
    }
}
