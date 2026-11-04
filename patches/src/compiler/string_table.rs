//! The compiler's string table: a global array of string entries (null: a free slot) and its count, filled by the
//! string intern and thinned by the `.dmb` save, which frees unused strings. Features that patch around the save
//! find it here (Windows).

use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The `.dmb` save's store that frees a slot (the table's only store of a null).
pub(crate) struct FreeStore {
    /// The global holding the table's address.
    pub(crate) table: usize,
    /// `mov dword ptr [eax+ecx*4], 0` (7 bytes): eax = the table, ecx = the slot.
    pub(crate) at: usize,
}

/// Finds the [`FreeStore`]: `mov eax,[table] ; mov ecx,[ebp-x] ; push 0 ; push edi ; mov dword [eax+ecx*4],0 ; call`.
pub(crate) fn free_store(cx: &Ctx) -> Result<&FreeStore> {
    cx.shared("string-table free store", |cx| {
        let core = cx.core();
        let m =
            core.find_unique("A1 ?? ?? ?? ?? 8B 4D ?? 6A 00 57 C7 04 88 00 00 00 00 E8", "string-table free store")?;
        Ok(FreeStore { table: core.ptr_at(m + 1)?, at: m + 11 })
    })
}

/// The `.dmb` save's string-table function: the last function entry (`int3` padding, then `push ebp ; mov ebp,esp ;
/// sub esp,..`) in the 0x400 bytes before the [`FreeStore`]. It must use the table near its start.
pub(crate) fn save(cx: &Ctx) -> Result<usize> {
    cx.shared("string-table save", |cx| {
        let core = cx.core();
        let free = free_store(cx)?;
        let entries = core.find_bytes_in(free.at - 0x400, 0x400, &[0xCC, 0x55, 0x8B, 0xEC, 0x83, 0xEC])?;
        let save = entries.as_slice().last().ok_or("string-table save: function entry not found")? + 1;
        ensure!(core.mentions(save, 0x200, free.table)?, "string-table save: doesn't use the string table");
        Ok(save)
    })
    .copied()
}
