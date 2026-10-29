//! `jsonutf` (forum report 2905231): `json_encode()` keeps UTF-8 instead of escaping every non-ASCII character as
//! `\uXXXX`.
//!
//! `StringEditor::AddJsonEncodedUtf8` decodes each byte >= 0x80 as UTF-8 and writes an escape. The fix sends those
//! bytes down its plain-copy branch, so they pass through. Bytes are copied unchecked: for the invalid code points
//! BYOND lets through (`ascii2text(0xD800)`, `url_decode("%ED%A0%80")`), the output is invalid UTF-8 where stock wrote
//! a wrong escape.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static JSONUTF: Feature = Feature::fix(
    Hosts::SERVER,
    "jsonutf",
    "json_encode keeps UTF-8",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);
