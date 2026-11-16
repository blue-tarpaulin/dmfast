//! Selections of opt-in fixes and DreamSeeker patches, as dmfast, the installer and `--client` read them: entries
//! separated by commas or spaces, each a key, a tier name, `experimental`, or `-key` to leave one out. Kept in
//! [`CONFIG_FILE`], an INI file next to dmfast's library.

/// dmfast's settings file, next to its library: `fixes` is dmfast's selection, `client` the installer's `--client`.
pub const CONFIG_FILE: &str = "dmfast.ini";

/// The entry that lets Experimental features turn on.
pub const EXPERIMENTAL: &str = "experimental";

/// Fix tiers, as in BUGFIXES.md: `safe` only changes the buggy case; `extended` changes behaviour a working game
/// could rely on; `breaking` turns code stock accepts into errors or changes results. A tier includes lower ones.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Safe,
    Extended,
    Breaking,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Safe, Tier::Extended, Tier::Breaking];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Safe => "safe",
            Tier::Extended => "extended",
            Tier::Breaking => "breaking",
        }
    }

    pub fn parse(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// A selection's entries, as written.
pub fn split(value: &str) -> impl Iterator<Item = &str> {
    value.split(|c: char| c == ',' || c.is_whitespace()).filter(|e| !e.is_empty())
}

/// Whether `entries` hold `entry`.
pub fn has(entries: &[String], entry: &str) -> bool {
    entries.iter().any(|e| e == entry)
}

/// Whether `entries` leave `key` out (`-key`).
pub fn left_out(entries: &[String], key: &str) -> bool {
    entries.iter().any(|e| e.strip_prefix('-') == Some(key))
}

/// Whether `entries` name `tier` or a riskier one.
pub fn by_tier(entries: &[String], tier: Tier) -> bool {
    Tier::ALL.iter().any(|&t| t >= tier && has(entries, t.name()))
}

/// Whether an INI line sets `key` (comments and surrounding spaces ignored).
pub fn ini_sets(line: &str, key: &str) -> bool {
    ini_pair(line).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key))
}

/// The value of `key` in INI text (any section; `;` and `#` start comments; the last one wins).
pub fn ini_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().rev().filter_map(ini_pair).find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

/// A line's key and value, comments and surrounding spaces dropped.
fn ini_pair(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split([';', '#']).next().unwrap_or("").split_once('=')?;
    Some((k.trim(), v.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(value: &str) -> Vec<String> {
        split(value).map(str::to_owned).collect()
    }

    #[test]
    fn entries_and_tiers() {
        assert_eq!(sel(" safe,,-a  b,"), ["safe", "-a", "b"]);
        let s = sel("extended, -a");
        assert!(left_out(&s, "a") && !left_out(&s, "b") && has(&s, "-a"));
        assert!(by_tier(&s, Tier::Safe) && by_tier(&s, Tier::Extended) && !by_tier(&s, Tier::Breaking));
        assert_eq!(Tier::parse("breaking"), Some(Tier::Breaking));
        assert_eq!(Tier::parse("none"), None);
    }

    #[test]
    fn ini_reads_the_last_setting() {
        let text = "[dmfast]\nfixes = safe ; old\n; fixes = breaking\nFIXES= extended # new\nclient=\n";
        assert_eq!(ini_value(text, "fixes"), Some("extended"));
        assert_eq!(ini_value(text, "client"), Some(""));
        assert_eq!(ini_value(text, "other"), None);
        assert!(ini_sets(" Fixes = x", "fixes") && !ini_sets("; fixes = x", "fixes"));
    }
}
