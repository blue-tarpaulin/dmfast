fn main() {
    // The private submodule's patches, unless this is the open-source build (`<version>-oss`; `DMFAST_EDITION`).
    println!("cargo::rustc-check-cfg=cfg(dmfast_private)");
    let private = dmfast_format::edition::private(std::path::Path::new(".."));
    if private {
        println!("cargo:rustc-cfg=dmfast_private");
    }
    println!("cargo:rustc-env=DMFAST_VERSION={}", dmfast_format::edition::version(private));
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
