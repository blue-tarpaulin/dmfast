//! `author` on the test fixtures (`fixture/counter.rs` with `counter.spec`, `fixture/every.rs`, `fixture/multi.rs`,
//! and `fixture/nonzero.rs`) and spec errors. Every patch's code starts with the prelude's functions, so the tests
//! look at the entry function's bytes and its fixups relative to it.

use std::path::Path;
use std::sync::LazyLock;

use dmfast_format::manifest::Field;
use dmfast_seeker::author::{compile, from_object};
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
static MULTI: LazyLock<Vec<u8>> = LazyLock::new(|| compiled("multi"));
static INDEX: LazyLock<Vec<u8>> = LazyLock::new(|| compiled("index"));

/// The entry function's first `len` bytes, and the fixups in them with offsets (and `Body` targets) relative to the
/// entry. The code's other fixups (the prelude's) point into the code.
fn at_entry(p: &Patch, len: usize) -> (Vec<u8>, Vec<Fixup>) {
    let entry = p.entry as usize;
    let range = entry..entry + len;
    let mut fixups = Vec::new();
    for f in &p.fixups {
        if range.contains(&(f.at as usize)) {
            let target = match f.target {
                Ref::Body(o) => Ref::Body(o - p.entry),
                ref t => t.clone(),
            };
            fixups.push(Fixup { at: f.at - p.entry, field: f.field, target });
        } else {
            assert!(matches!(f.target, Ref::Body(o) if (o as usize) < p.body.len()), "a prelude fixup: {f:?}");
        }
    }
    (p.body[range].to_vec(), fixups)
}

#[test]
fn counter_fixture() {
    let obj = COUNTER.as_slice();
    let patches = from_object(obj, SPEC, "counter").unwrap();
    let [p] = &patches[..] else { panic!("one patch: {patches:?}") };
    let stripped = Patch { body: Vec::new(), entry: 0, fixups: Vec::new(), ..p.clone() };
    assert_eq!(
        stripped,
        Patch {
            key: "counter".to_string(),
            tier: Tier::Safe,
            name: "counter".to_string(),
            about: "Counts DMString::DMString() calls in its data (test only).".to_string(),
            dll: Dll::Core,
            site: Place::Export("??0DMString@@QAE@XZ".to_string()),
            displace: 6,
            body: Vec::new(),
            entry: 0,
            fixups: Vec::new(),
            data: 4,
            consts: Vec::new(),
            const_fixups: Vec::new(),
        }
    );
    // lock inc dword ptr [dmf_data]; jmp dmf_original
    let (bytes, fixups) = at_entry(p, 12);
    assert_eq!(bytes, [0xF0, 0xFF, 0x05, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0]);
    assert_eq!(
        fixups,
        [
            Fixup { at: 3, field: Field::Abs32, target: Ref::Data(0) },
            Fixup { at: 8, field: Field::Rel32, target: Ref::Original(0) },
        ]
    );
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
    let (bytes, fixups) = at_entry(p, 25);
    assert_eq!(&bytes[..4], [0x85, 0xC0, 0x74, 0x05]);
    assert_eq!(
        fixups,
        [
            Fixup { at: 5, field: Field::Rel32, target: Ref::Code(Place::Export("?helper@@YAXXZ".into())) },
            Fixup { at: 11, field: Field::Abs32, target: Ref::Body(9) },
            Fixup { at: 16, field: Field::Abs32, target: Ref::Const(0) },
            Fixup { at: 21, field: Field::Rel32, target: Ref::Resume },
        ]
    );
}

#[test]
fn rust_functions_statics_and_direct3d() {
    let obj = MULTI.as_slice();
    let spec = "patch multi byondwin.dll
about test
tier safe
site export ?a@@YAXXZ
displace 5
body multi
";
    let patches = from_object(obj, spec, "multi").unwrap();
    let [p] = &patches[..] else { panic!("one patch: {patches:?}") };
    let mut data: Vec<u32> =
        p.fixups.iter().filter_map(|f| if let Ref::Data(o) = f.target { Some(o) } else { None }).collect();
    data.sort();
    data.dedup();
    assert_eq!(data, [0, 4]);
    assert!(p.fixups.iter().filter(|f| matches!(f.target, Ref::Data(_))).all(|f| f.field == Field::Abs32));
    // `bump`'s address is taken (the only `Abs32` to the code), and it is called: not the entry.
    let address = p.fixups.iter().find(|f| f.field == Field::Abs32 && matches!(f.target, Ref::Body(_))).unwrap();
    assert_ne!(address.target, Ref::Body(p.entry));
    assert!(p.fixups.iter().any(|f| f.field == Field::Rel32 && f.target == address.target), "bump is called");
    // com3: one call to the helper's `dmf_com`.
    let com: Vec<_> = p.fixups.iter().filter(|f| matches!(f.target, Ref::Com | Ref::ComTail)).collect();
    assert_eq!(com.len(), 1);
    assert_eq!((com[0].field, &com[0].target), (Field::Rel32, &Ref::Com));
    // push 3 ; push 2 ; push n ; push device ; push 17 ; call dmf_com
    assert_eq!(p.body[com[0].at as usize - 1], 0xE8);
}

#[test]
fn nonzero_static() {
    let obj = compiled("nonzero");
    let spec = "patch nonzero byondwin.dll\nabout test\ntier safe\nsite export a\ndisplace 5\nbody nonzero\n";
    let error = from_object(&obj, spec, "nonzero").expect_err("a static that isn't zero");
    assert!(error.starts_with("line 6: ") && error.contains(".data"), "{error}");
}

#[test]
fn fingerprint_sites() {
    let obj = COUNTER.as_slice();
    let spec = "patch counter byondcore.dll
about test
site fingerprint 0x40 0x0123456789abcdef c7 01 ?? 00
displace 6
body counter
data 4
tier safe
";
    let patches = from_object(obj, spec, "counter").unwrap();
    let fingerprint = Place::Fingerprint { prefix: "C7 01 ?? 00".into(), len: 0x40, hash: 0x0123_4567_89ab_cdef };
    assert_eq!(patches[0].site, fingerprint);
    let error = from_object(obj, &spec.replace("0x0123456789abcdef", "12345"), "counter").unwrap_err();
    assert!(error.starts_with("line 3: ") && error.contains("hash"), "{error}");
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
    let not_a_function = PLAIN_SPEC.replace("body counter", "body dmf_data");
    assert!(error_at(obj, &not_a_function, 5).contains("dmf_data"));
}

#[test]
fn only_reached_code_and_core_panics_trap() {
    let spec = "patch index byondcore.dll\nabout test\ntier safe\nsite export x\ndisplace 5\nbody index\n";
    let p = &from_object(INDEX.as_slice(), spec, "index").unwrap()[0];
    // The prelude's memcpy and friends aren't reached, so they aren't in the code.
    assert!(p.body.len() < 64, "{} bytes of code", p.body.len());
    // The bounds check calls the trap: a `ud2` in the code.
    let trap = p.fixups.iter().find_map(|f| match f.target {
        Ref::Body(at) if at != p.entry => Some(at as usize),
        _ => None,
    });
    assert_eq!(trap.map(|at| &p.body[at..at + 2]), Some(&[0x0F, 0x0B][..]));
    // The panic's location is a constant with the address of its file name, named by its folder (`fixture`).
    assert_eq!(p.const_fixups.len(), 1);
    let (field, to) = p.const_fixups[0];
    assert_eq!(&p.consts[field as usize..field as usize + 4], [0; 4]);
    assert!(p.consts[to as usize..].starts_with(br"fixture\index.rs"), "{:?}", String::from_utf8_lossy(&p.consts));
}
