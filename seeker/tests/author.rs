//! `author` on the test fixtures (`fixture/counter.rs` with `counter.spec`, and `fixture/every.rs`) and spec errors.

#![cfg(feature = "build")]

use std::path::Path;
use std::sync::LazyLock;

use dmfast_seeker::author::{compile, from_object};
use dmfast_seeker::format::Field;
use dmfast_seeker::patch::{Dll, Fixup, Patch, Place, Ref, Tier};

const SPEC: &str = include_str!("fixture/counter.spec");
/// The counter patch without comments: `patch` on line 1, `body` on line 5.
const PLAIN_SPEC: &str = "patch counter byondcore.dll
about test
site export ??0DMString@@QAE@XZ
displace 6
body counter
data 4
tier safe
";

fn rustc() -> String {
    std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string())
}

/// `fixture/<name>.rs` compiled as the installer compiles private patches (needs the i686 target, as the installer's
/// build does: `rustup target add i686-pc-windows-msvc`).
fn compiled(name: &str) -> Vec<u8> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixture").join(format!("{name}.rs"));
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&out).expect("creating the fixture's output folder");
    let obj = compile(&rustc(), &fixture, &out, "1.2.3").expect("compiling the fixture");
    std::fs::read(obj).expect("reading the fixture's object")
}

static COUNTER: LazyLock<Vec<u8>> = LazyLock::new(|| compiled("counter"));
static EVERY: LazyLock<Vec<u8>> = LazyLock::new(|| compiled("every"));

fn counter() -> Patch {
    Patch {
        key: "counter".to_string(),
        tier: Tier::Safe,
        name: "counter".to_string(),
        about: "Counts DMString::DMString() calls in its data (test only).".to_string(),
        dll: Dll::Core,
        site: Place::Export("??0DMString@@QAE@XZ".to_string()),
        displace: 6,
        // lock inc dword ptr [dmf_data]; jmp dmf_original
        body: vec![0xF0, 0xFF, 0x05, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0],
        fixups: vec![
            Fixup { at: 3, field: Field::Abs32, target: Ref::Data(0) },
            Fixup { at: 8, field: Field::Rel32, target: Ref::Original },
        ],
        data: 4,
        consts: Vec::new(),
    }
}

#[test]
fn counter_fixture() {
    let obj = COUNTER.as_slice();
    assert_eq!(from_object(obj, SPEC, "counter").unwrap(), vec![counter()]);
}

#[test]
fn every_kind_of_reference() {
    let obj = EVERY.as_slice();
    let spec = "patch every byondcore.dll
about test
tier extended
site export ?a@@YAXXZ
site pattern -2 8B ?? 55
displace 5
body every
ref helper code export ?helper@@YAXXZ
";
    let patches = from_object(obj, spec, "every").unwrap();
    let names: Vec<_> = patches.iter().map(|p| (p.name.as_str(), p.site.clone())).collect();
    assert_eq!(
        names,
        [("every", Place::Export("?a@@YAXXZ".into())), ("every-2", Place::Pattern("8B ?? 55".into(), -2)),]
    );
    let p = &patches[0];
    assert_eq!((p.key.as_str(), p.tier), ("every", Tier::Extended));
    assert_eq!(p.consts, b"hi\0");
    // test eax, eax ; jz 2f ; call helper ; 2: lea edx, [2b] ; mov ecx, offset TEXT ; jmp dmf_resume
    assert_eq!(&p.body[..4], [0x85, 0xC0, 0x74, 0x05]);
    assert_eq!(
        p.fixups,
        [
            Fixup { at: 5, field: Field::Rel32, target: Ref::Code(Place::Export("?helper@@YAXXZ".into())) },
            Fixup { at: 11, field: Field::Abs32, target: Ref::Body(9) },
            Fixup { at: 16, field: Field::Abs32, target: Ref::Const(0) },
            Fixup { at: 21, field: Field::Rel32, target: Ref::Resume },
        ]
    );
}

/// `from_object`'s error for the counter's object and `spec`, checked to start with `line`.
fn error_at(obj: &[u8], spec: &str, line: usize) -> String {
    let error = from_object(obj, spec, "counter").expect_err("the spec should fail");
    assert!(error.starts_with(&format!("line {line}: ")), "{error}");
    error
}

#[test]
fn spec_errors() {
    let obj = COUNTER.as_slice();
    let unknown_symbol = PLAIN_SPEC.replace("body counter", "body nosuch");
    assert!(error_at(obj, &unknown_symbol, 5).contains("nosuch"));
    let unknown_directive = format!("{PLAIN_SPEC}frobnicate 1\n");
    assert!(error_at(obj, &unknown_directive, 8).contains("frobnicate"));
    let missing_body = PLAIN_SPEC.replace("body counter\n", "");
    assert!(error_at(obj, &missing_body, 1).contains("body"));
    let no_data = PLAIN_SPEC.replace("data 4", "data 0");
    assert!(error_at(obj, &no_data, 5).contains("dmf_data"));
}
