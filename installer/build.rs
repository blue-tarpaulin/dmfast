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
    let lib = build_x86(&root, &target_dir, triple, "dmfast", file);
    println!("cargo:rustc-env=DMFAST_LIB={}", lib.display());
    println!("cargo:rerun-if-changed={}", root.join("base").display());
    println!("cargo:rerun-if-changed={}", root.join("dmfast").display());
    println!("cargo:rerun-if-changed={}", root.join("patches").display());
    println!("cargo:rerun-if-changed={}", root.join("patches-private").display());
    write_bugfix_list(&root);
    if !windows {
        check_glibc(&lib);
        return;
    }
    println!("cargo:rerun-if-changed={}", root.join("vendor").join("mimalloc-x86").display());

    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "dmfast")
        .set("FileDescription", "Robusts BYOND's toolchain.")
        .set("CompanyName", "Greytide Worldwide")
        .set("LegalCopyright", "dmfast contributors; bundles mimalloc (MIT, Microsoft Corporation)")
        .set("OriginalFilename", "dmfast-install.exe")
        .set_manifest(&MANIFEST.replace("{version}", &format!("{}.0", env!("CARGO_PKG_VERSION"))));
    res.compile().expect("embedding version info / manifest");
}

/// Builds workspace package `package` for `triple` into `target_dir` and returns the path of `file`.
fn build_x86(root: &std::path::Path, target_dir: &std::path::Path, triple: &str, package: &str, file: &str) -> PathBuf {
    let status = Command::new(env::var("CARGO").unwrap())
        .args(["build", "--release", "--target", triple, "-p", package, "--target-dir"])
        .arg(target_dir)
        // The embedded copy needs no symbol table (development builds keep theirs for profiling).
        .args(["--config", "profile.release.strip=true"])
        .current_dir(root)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .status()
        .unwrap_or_else(|e| panic!("failed to run cargo for {package}: {e}"));
    assert!(status.success(), "building {file} failed");
    target_dir.join(triple).join("release").join(file)
}

/// `--help bugfixes`: the fix tables of BUGFIXES.md (name, key, tier, OS), written to
/// `$OUT_DIR/bugfixes.txt` so the list can't drift from the document.
fn write_bugfix_list(root: &std::path::Path) {
    let doc = root.join("BUGFIXES.md");
    println!("cargo:rerun-if-changed={}", doc.display());
    let text = std::fs::read_to_string(&doc).expect("reading BUGFIXES.md");
    let mut out = String::new();
    let mut section = None;
    let mut tier_now = String::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = Some(h.split(" (").next().unwrap_or(h).to_string());
            tier_now.clear();
            continue;
        }
        let Some(name) = &section else { continue };
        let cells: Vec<String> = line.split('|').map(|c| c.trim().replace('`', "")).collect();
        // "| Fix | Key | Tier | OS | ..." rows; skip the header and the |---| separator.
        if cells.len() < 6 || cells[1] == "Fix" || cells[1].starts_with('-') {
            continue;
        }
        let (fix, key, tier, os) = (&cells[1], &cells[2], &cells[3], &cells[4]);
        if tier_now.is_empty() {
            out.push_str(&format!("\n{name}\n"));
        }
        if *tier != tier_now {
            out.push_str(&format!("  -------- {tier} --------\n"));
            tier_now = tier.clone();
        }
        let os = if os == "Both" { String::new() } else { format!(" ({os} only)") };
        out.push_str(&format!("  {key:<12}{fix}{os}\n"));
    }
    assert!(out.contains("contline"), "no fix rows found in BUGFIXES.md");
    std::fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("bugfixes.txt"), out).unwrap();
}

/// Runs as the invoking user (it relaunches itself elevated only when the target needs it). `{version}`
/// becomes the package version as four parts.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="GreytideWorldwide.dmfast.install" version="{version}"/>
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
