//! [`crate::base::validate`]'s checks over [`crate::CONFIG`], and dmfast's own:
//! the dependency guards name only features, every fix has a test world.

use crate::CONFIG;
use crate::base::feature::Kind;
use crate::base::validate;

#[test]
fn keys_are_unique() {
    validate::keys_are_unique(&CONFIG);
}

#[test]
fn declarations_are_consistent() {
    validate::declarations_are_consistent(&CONFIG);
}

#[test]
fn no_cycles() {
    validate::no_cycles(&CONFIG);
}

/// Every key the dependency guards name (both OSes' tables in `patches-private/src/deps.rs`) is a feature.
#[test]
fn deps_keys_are_features() {
    let data = include_str!("../../patches-private/src/deps.rs");
    let mut n = 0;
    for list in data.split("keys: &[").skip(1) {
        let list = &list[..list.find(']').expect("keys list end")];
        for key in list.split(',').map(|k| k.trim().trim_matches('"')).filter(|k| !k.is_empty()) {
            n += 1;
            assert!(CONFIG.all().any(|f| f.key == key), "deps.rs names {key:?}, which is no feature");
        }
    }
    assert!(n > 0, "no keys found in deps.rs");
}

#[test]
fn no_dead_features() {
    validate::no_dead_features(&CONFIG);
}

/// Every fix has a test: `tests/fixes/<key>.dme`, or its own kit in `patches-private/tests/<key>/`.
#[test]
fn fixes_have_tests() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for f in CONFIG.all().filter(|f| f.kind == Kind::Fix) {
        let ok = root.join(format!("tests/fixes/{}.dme", f.key)).is_file()
            || root.join("patches-private/tests").join(f.key).is_dir();
        assert!(ok, "fix {}: no tests/fixes/{0}.dme", f.key);
    }
}
