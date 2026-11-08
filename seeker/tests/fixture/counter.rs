//! Test-only patch: counts calls to byondcore.dll's `DMString::DMString()` in its data, then runs the site's
//! original instructions. Its spec is `counter.spec`.

use core::arch::naked_asm;

use crate::prelude::*;

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn counter() {
    naked_asm!(
        "lock inc dword ptr [{data}]",
        "jmp {original}",
        data = sym dmf_data,
        original = sym dmf_original,
    )
}
