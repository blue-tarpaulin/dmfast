fn main() {
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
