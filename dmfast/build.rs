fn main() {
    // The private submodule's patches, if it is checked out.
    println!("cargo::rustc-check-cfg=cfg(dmfast_private)");
    println!("cargo:rerun-if-changed=../patches-private/src");
    if std::path::Path::new("../patches-private/src/lib.rs").exists() {
        println!("cargo:rustc-cfg=dmfast_private");
    }
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            let mut res = winresource::WindowsResource::new();
            res.set("ProductName", "dmfast")
                .set("FileDescription", "Robusts BYOND's toolchain.")
                .set("CompanyName", "Greytide Worldwide")
                .set("LegalCopyright", "dmfast contributors")
                .set("OriginalFilename", "dmfast.dll");
            res.compile().expect("embedding version info");
        }
        // Allows text relocations, which the hook stubs need (they address their state absolutely,
        // like the code they patch in libbyond.so, which is itself built with text relocations).
        Ok("linux") => println!("cargo:rustc-link-arg=-Wl,-z,notext"),
        _ => {}
    }
}
