//! `inputop`, Windows: the null first arg is written through a pointer to the arg array.

use super::point_at;
use crate::base::Result;
use crate::base::feature::Ctx;

/// mov eax,[ebp+stale] ; mov esi,[context] ; push 2 ; mov dword [eax],0 ; mov dword [eax+4],0 (the first arg's
/// null) ; ... (the second arg) ; lea eax,[ebp+args] (the arg array).
const ROUTE: &str = "8B 85 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 6A 02 C7 00 00 00 00 00 C7 40 04 00 00 00 00 0F B7 56 ?? 8B 4E \
                     ?? 8B 44 D1 F8 8B 4C D1 FC 89 85 ?? ?? ?? ?? 8D 85";
const ARGS_LEA: usize = 48;

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(ROUTE, "overloaded input: first arg")?;
    point_at(core, at, at + ARGS_LEA + 2)
}
