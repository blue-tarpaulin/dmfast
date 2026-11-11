//! `httpexport` (Windows): `world.Export()` of an `http://` URL no longer fails or crashes on a big or fast response.
//!
//! An HTTP request (`HttpRequest2`, and `world.Export`'s `WorldExportHttpRequest`) is reference counted at `+4`.
//! WinHTTP's callback thread adds a reference for each chunk it hands to BYOND's task thread, and that thread drops it
//! when the chunk is handled, both without `lock`. A lost update frees the request mid-download: the download is cut
//! off and freed memory is used, which crashes the server. Each update becomes its `lock` form. DreamSeeker has the
//! same bug in `preload_rsc` downloads, fixed by the `rscdownload` client patch (CLIENT.md).

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(windows)]
mod windows;

pub static HTTPEXPORT: Feature = Feature::fix(
    Hosts::SERVER,
    "httpexport",
    "HTTP request reference counts are atomic",
    Tier::Safe,
    install!(windows: windows::install),
);
