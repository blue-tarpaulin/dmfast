//! `contline` (forum report 2990026): a `\` at the end of a line continues it across one newline only.
//!
//! After `\` and a newline, the tokenizer skips whitespace with `isspace`, which includes newlines, so stock joins
//! any number of blank lines. The fix stops the skip at the next newline. Strings and file names are unaffected.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static CONTLINE: Feature = Feature::fix(
    Hosts::COMPILER,
    "contline",
    "backslash continues one line only",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);
