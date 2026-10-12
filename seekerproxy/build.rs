//! No C runtime: the DLL imports kernel32 only (it is loaded by every program that loads byondwin.dll,
//! and DreamSeeker's module scan is touchy about what is around).
fn main() {
    println!("cargo:rustc-cdylib-link-arg=/NODEFAULTLIB");
    println!("cargo:rustc-cdylib-link-arg=/ENTRY:dmfast_entry");
}
