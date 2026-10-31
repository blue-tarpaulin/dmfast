//! `regexstart` (forum report 2885674): `regex.Find(text, start)` no longer ignores `start` for a global regex that
//! last searched the same text (it carried on from `regex.next`).
//!
//! The built-in `/regex` DM source fills an omitted `start` in as 1, so the runtime can't tell it from an explicit
//! one. Its chunk is replaced by a copy where `Find` and `Find_char` default `start` to null and set `next = start`
//! when it's given. The procs are compiled into the `.dmb`, so the fix needs only the compiler.

use std::sync::OnceLock;

use super::replace_source;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::{Result, bail};

pub static REGEXSTART: Feature = Feature::fix(
    Hosts::COMPILER,
    "regexstart",
    "regex.Find(text, start) restarts a global regex",
    Tier::Breaking,
    install!(windows: install, linux: install),
);

const MARK: &[u8] = b"\n#define REGEX_QUOTE(a) regex((a), 1)\n";
const REPLACE: [(&str, &str); 2] = [
    (
        "\tproc/Find(text, start=1, end)\n\t\treturn findtext(text, src, start, end)\n",
        "\tproc/Find(text, start, end)\n\t\tif(isnull(start)) start = 1\n\t\telse next = start\n\t\treturn findtext(text, src, start, end)\n",
    ),
    (
        "\tproc/Find_char(text, start=1, end)\n\t\treturn findtext_char(text, src, start, end)\n",
        "\tproc/Find_char(text, start, end)\n\t\tif(isnull(start)) start = 1\n\t\telse next = start\n\t\treturn findtext_char(text, src, start, end)\n",
    ),
];

static FIXED: OnceLock<Vec<u8>> = OnceLock::new();

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let slot = replace_source(core, MARK, "regex DM source", &FIXED, |text| {
        // The replacement sets `next`: the datum must declare it, or the built-ins would not compile.
        let declares =
            |l: &str| l.strip_prefix("\tvar/next").is_some_and(|r| r.is_empty() || r.starts_with(['\t', ' ']));
        if !text.lines().any(declares) {
            bail!("regex DM source: no var/next");
        }
        let mut fixed = text.to_string();
        for (old, new) in REPLACE {
            if fixed.matches(old).count() != 1 {
                bail!("regex DM source: Find not as expected");
            }
            fixed = fixed.replacen(old, new, 1);
        }
        Ok(fixed)
    })?;
    Ok(format!("table entry at +{:#x}", slot - core.base))
}
