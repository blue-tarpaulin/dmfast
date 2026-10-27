//! Checks over a [`Config`]'s feature lists, for the tests of the crate using this one (one `#[test]` per check,
//! with its configuration). Conflicts between features (overlapping writes, or code one relies on that another
//! changes: [`crate::ledger`]) can only show at install, against a real BYOND: run with every feature selected
//! (`DMFAST_FIXES=breaking,experimental DMFAST_STATS=1`) and look for `conflicts with`.

use std::collections::HashSet;

use crate::driver::Config;
use crate::feature::{Feature, Host, Kind, OLDEST_SUPPORTED};

/// Keys are unique and `a-z0-9`.
pub fn keys_are_unique(config: &Config) {
    let mut seen = HashSet::new();
    for f in config.all() {
        assert!(seen.insert(f.key), "key {} declared twice", f.key);
        assert!(f.key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()), "key {}: a-z0-9 only", f.key);
    }
}

/// Every feature installs on some OS, has a tier iff it is a fix, and its `after`/`requires` share its program.
pub fn declarations_are_consistent(config: &Config) {
    for f in config.all() {
        assert!(f.install.os != 0, "{}: no install function on any OS", f.key);
        match f.kind {
            Kind::Perf => assert!(f.tier.is_none(), "{}: a speed patch has no tier", f.key),
            Kind::Fix => assert!(f.tier.is_some(), "{}: a fix needs a tier", f.key),
        }
        for d in f.after.iter().chain(f.requires) {
            assert!(
                Host::ALL.iter().any(|&h| f.hosts.has(h) && d.hosts.has(h)),
                "{} -> {}: no program has both",
                f.key,
                d.key
            );
            assert!(config.all().any(|x| std::ptr::eq(x, *d)), "{} -> {}: not in any list", f.key, d.key);
        }
    }
}

/// Every `after`/`requires` edge points backwards in install order (so no cycles).
pub fn no_cycles(config: &Config) {
    for host in Host::ALL {
        let order = config.ordered(host);
        let at = |f: &Feature| order.iter().position(|o| std::ptr::eq(*o, f));
        for f in &order {
            for d in f.after.iter().chain(f.requires) {
                if let (Some(i), Some(j)) = (at(f), at(d)) {
                    assert!(j < i, "{host:?}: {} must install after {} (cycle)", f.key, d.key);
                }
            }
        }
    }
}

/// No feature is for a bug BYOND fixed in every supported build, and each `fixed_in` links its bug report.
pub fn no_dead_features(config: &Config) {
    for f in config.all() {
        if let Some((build, report)) = f.byond.fixed_in {
            assert!(build > OLDEST_SUPPORTED, "{}: BYOND fixed it in every supported build: delete it", f.key);
            assert!(report.starts_with("https://"), "{}: fixed_in needs the bug report's URL", f.key);
        }
    }
}

#[cfg(test)]
fn sel(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|e| e.to_string()).collect()
}

#[test]
fn selection_rule() {
    use crate::driver::select::wanted;
    use crate::feature::{Hosts, Install, Status, Tier};
    const NONE: Install = Install { os: 1, run: None };
    let perf = Feature::perf(Hosts::SERVER, "p", "", NONE);
    let safe = Feature::fix(Hosts::SERVER, "s", "", Tier::Safe, NONE);
    let ext = Feature::fix(Hosts::SERVER, "e", "", Tier::Extended, NONE);
    let brk = Feature::fix(Hosts::SERVER, "b", "", Tier::Breaking, NONE);
    let xfix = Feature::fix(Hosts::SERVER, "x", "", Tier::Safe, NONE).experimental();
    let xperf = Feature::perf(Hosts::SERVER, "y", "", NONE).experimental();
    let on = Feature::fix(Hosts::SERVER, "o", "", Tier::Safe, NONE).default_on();
    assert!(on.on_by_default);
    let w = |f: &Feature, e: &[&str]| wanted(f, &sel(e));
    assert!(w(&perf, &[]) && !w(&perf, &["-p"]));
    assert!(!w(&safe, &[]) && w(&safe, &["safe"]) && w(&safe, &["s"]) && !w(&safe, &["safe", "-s"]));
    assert!(!w(&ext, &["safe"]) && w(&ext, &["extended"]) && w(&ext, &["breaking"]));
    assert!(!w(&brk, &["extended"]) && w(&brk, &["breaking"]));
    // Tiers alone never turn on an Experimental feature; `experimental` alone turns on no fix.
    assert!(!w(&xfix, &["breaking"]) && !w(&xfix, &["experimental"]) && w(&xfix, &["safe", "experimental"]));
    assert!(w(&xfix, &["x"]) && !w(&xperf, &[]) && w(&xperf, &["experimental"]));
    assert!(w(&on, &[]) && !w(&on, &["-o"]));
    let dep = Feature::fix(Hosts::SERVER, "d", "", Tier::Safe, NONE).deprecated("gone");
    assert_eq!(dep.status, Status::Deprecated("gone"));
    assert_eq!(Feature::perf(Hosts::SERVER, "n", "", NONE).since(516_1674).byond.since, 516_1674);
}
