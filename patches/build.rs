//! Windows: embeds an `asInvoker` manifest in the test executable.
//! Without one, Windows asks for elevation to run an .exe named with 'patch'.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='asInvoker'");
    }
}
