//! `callleak`, Linux: both call_ext() opcodes share one buffer, whose address a slot of the interpreter's frame holds
//! (`push [ebp-x]`).

use crate::base::code::Foreign;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The buffer's allocation: sub esp,1Ch ; mov edx,[esp+20h] (the buffer) ; mov ecx,[esp+24h] (the count) ;
/// mov eax,[edx+used] ; add ecx,eax ; cmp ecx,inline slots ; ja block ; mov dword [edx+block],0 ;
/// lea eax,[edx+eax*8] ; mov [edx+used],ecx
pub(super) const ALLOC: &str = "83 EC 1C 8B 54 24 20 8B 4C 24 24 8B 42 ?? 01 C1 83 F9 ?? 77 ?? C7 42 ?? 00 00 00 00 8D 04 \
                                C2 89 4A ??";
/// Its `used` and `block` offsets.
pub(super) const USED: usize = 13;
pub(super) const BLOCK: usize = 23;
/// Its `ja block`; the block: ... ; shl eax,3 ; push eax ; call malloc
const JA_BLOCK: usize = 19;
const MALLOC: &str = "C1 E0 03 50 E8";
/// An opcode's allocation: push [ebp-x] (the buffer) ; call alloc
pub(super) const ALLOC_CALL: &str = "FF B5 ?? ?? ?? ?? E8";
/// The buffer's cleanup, after the call and in the handler for a throwing call: push [ebp-x] ; call cleanup
pub(super) const CLEANUP: [&str; 1] = ["FF B5 ?? ?? ?? ?? E8"];

/// Whether code starts with a bare return.
pub(super) fn is_ret(code: &[u8]) -> bool {
    code.first() == Some(&0xC3)
}

/// Checks the allocation takes its block from the process's `malloc`, and returns its `free`.
pub(super) fn free(core: &Image, alloc: usize) -> Result<Foreign<unsafe extern "C" fn(usize)>> {
    let block = core.branch_at(alloc + JA_BLOCK)?.target;
    let call = core.find_unique_in(block, 0x20, MALLOC, "call_ext(arglist()) buffer block")? + 4;
    ensure!(
        core.call_target(call)? == libc::malloc as *const () as usize,
        "call_ext(arglist()): the buffer doesn't malloc"
    );
    Ok(Foreign::at(libc::free as *const ()))
}
