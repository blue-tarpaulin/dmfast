//! Builds the 32-bit dmfast library for the installer's target OS (it must match BYOND's x86
//! processes regardless of the installer's own architecture) and hands its path to `include_bytes!`
//! via `DMFAST_LIB`. On Windows also embeds the installer's version information and manifest; on Linux
//! checks that the library loads wherever BYOND does (no glibc symbol newer than [`GLIBC_FLOOR`]).

use std::env;
use std::path::PathBuf;
use std::process::Command;

use object::read::elf::{ElfFile32, FileHeader};

/// BYOND 516's libbyond.so and DreamMaker need glibc 2.34; libdmfast.so must not need more, or it fails
/// to load on distributions BYOND itself supports. Build on a system with glibc <= 2.34 (Debian 12).
const GLIBC_FLOOR: (u32, u32) = (2, 34);

fn main() {
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let (triple, file) =
        if windows { ("i686-pc-windows-msvc", "dmfast.dll") } else { ("i686-unknown-linux-gnu", "libdmfast.so") };
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let target_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("dmfast-target");
    let status = Command::new(env::var("CARGO").unwrap())
        .args(["build", "--release", "--target", triple, "-p", "dmfast", "--target-dir"])
        .arg(&target_dir)
        // The embedded copy needs no symbol table (development builds keep theirs for profiling).
        .args(["--config", "profile.release.strip=true"])
        .current_dir(&root)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .status()
        .expect("failed to run cargo for dmfast");
    assert!(status.success(), "building {file} failed");
    let lib = target_dir.join(triple).join("release").join(file);
    println!("cargo:rustc-env=DMFAST_LIB={}", lib.display());
    println!("cargo:rerun-if-changed={}", root.join("patches").display());
    if !windows {
        check_glibc(&lib);
        return;
    }
    println!("cargo:rerun-if-changed={}", root.join("vendor").join("mimalloc-x86").display());

    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "dmfast")
        .set("FileDescription", "Robusts BYOND's compiler.")
        .set("CompanyName", "Greytide Worldwide")
        .set("LegalCopyright", "dmfast contributors; bundles mimalloc (MIT, Microsoft Corporation)")
        .set("OriginalFilename", "dmfast-install.exe")
        .set_manifest(MANIFEST);
    res.compile().expect("embedding version info / manifest");
}

/// Runs as the invoking user (it relaunches itself elevated only when the target needs it).
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="GreytideWorldwide.dmfast.install" version="4.6.0.0"/>
  <description>dmfast installer</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

/// The glibc versions `lib` requires (its `.gnu.version_r` entries for libc), checked against the floor.
fn check_glibc(lib: &std::path::Path) {
    let data = std::fs::read(lib).expect("reading libdmfast.so");
    let file = ElfFile32::<object::Endianness>::parse(&*data).expect("parsing libdmfast.so");
    let endian = file.endian();
    let sections = file.elf_header().sections(endian, &*data).expect("section table");
    let Some((mut needs, link)) = sections.gnu_verneed(endian, &*data).expect("version needs") else {
        return;
    };
    let strings = sections.strings(endian, &*data, link).expect("version strings");
    let mut too_new = Vec::new();
    while let Some((_, mut aux)) = needs.next().expect("version need") {
        while let Some(a) = aux.next().expect("version need entry") {
            let name = String::from_utf8_lossy(a.name(endian, strings).expect("version name")).into_owned();
            let Some(ver) = name.strip_prefix("GLIBC_") else { continue };
            let mut parts = ver.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
            let v = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
            if v > GLIBC_FLOOR {
                too_new.push(name);
            }
        }
    }
    assert!(
        too_new.is_empty(),
        "libdmfast.so needs {too_new:?}, newer than glibc {}.{} (BYOND's floor); build on an older system",
        GLIBC_FLOOR.0,
        GLIBC_FLOOR.1
    );
}
