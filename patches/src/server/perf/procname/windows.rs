//! `procname`, Windows: byondcore's callers of the name lookup call [`super::lookup`].

use crate::base::Result;
use crate::base::feature::Ctx;

/// The whole lookup (cdecl: map, name, depth, flag): the binary search over 12-byte entries, then the flag and
/// depth checks and the override.
pub(in crate::server::perf) const LOOKUP: &str = "55 8B EC 8B 45 08 33 D2 53 56 57 0F B7 70 04 8B 38 89 7D 08 85 F6 74 2B 8B 5D 0C 0F 1F 44 00 00 \
     8D 04 16 D1 E8 8D 0C 40 8B 3C 8F 3B DF 74 1E 8B C8 0F 43 CE 40 3B DF 8B F1 8B 7D 08 0F 43 D0 3B \
     D6 72 DD 5F 5E B8 FF FF 00 00 5B 5D C3 8B 4D 08 8D 04 40 80 7C 81 0A 00 8D 14 81 74 06 80 7D 14 \
     00 75 E0 8A 4D 10 0F B6 C1 66 3B 42 08 73 D4 8B 42 04 0F B6 C9 5F 5E 5B 8B 04 88 5D C3";

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(LOOKUP, "proc name lookup")?;
    super::redirect(core, f)
}
