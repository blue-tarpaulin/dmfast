//! Linux: allows text relocations in the unit test executable, as `patches-private/build.rs` does for dmfast.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-z,notext");
    }
}
