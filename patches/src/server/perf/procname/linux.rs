//! `procname`, Linux: libbyond's callers of the name lookup call [`super::lookup`].

use crate::base::Result;
use crate::base::feature::Ctx;

/// The whole lookup (cdecl: map, name, depth, flag): the binary search over 12-byte entries, then the flag and
/// depth checks and the override.
pub(in crate::server::perf) const LOOKUP: &str = "55 57 56 53 31 DB 83 EC 04 8B 44 24 18 8B 74 24 24 8B 7C 24 1C 8B 6C 24 20 89 34 24 0F B7 50 04 \
     8B 30 EB 1B 8D 74 26 00 8D 04 13 89 C1 83 E0 FE D1 E9 01 C8 8D 04 86 39 38 74 1D 76 53 89 CA 39 \
     D3 72 E5 BA FF FF 00 00 83 C4 04 89 D0 5B 5E 5F 5D C3 8D B6 00 00 00 00 80 78 0A 00 74 06 80 3C \
     24 00 75 DF 89 EB 0F B7 48 08 BA FF FF 00 00 0F B6 EB 39 CD 7D D2 8B 40 04 8B 14 A8 83 C4 04 5B \
     5E 89 D0 5F 5D C3 8D B4 26 00 00 00 00 8D 76 00 8D 59 01 EB AA";

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(LOOKUP, "proc name lookup")?;
    super::redirect(core, f)
}
