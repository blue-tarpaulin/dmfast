//! Test-only patch body with every kind of reference: a `ref` call, a label's address, a constant, and a built-in.

use core::arch::naked_asm;

use crate::prelude::*;

unsafe extern "C" {
    fn helper();
}

text!(TEXT = "hi");

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn every() {
    naked_asm!(
        "test eax, eax",
        "jz 2f",
        "call {helper}",
        "2:",
        "lea edx, [2b]",
        "mov ecx, offset {text}",
        "jmp {resume}",
        helper = sym helper,
        text = sym TEXT,
        resume = sym dmf_resume,
    )
}
