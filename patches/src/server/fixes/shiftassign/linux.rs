//! `shiftassign`, Linux: each shift-assign overload case ends in a stub that restores the opcode's position, which
//! the case overwrote, and goes on to `+=`'s write.

use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// `+=`'s overload case: mov ecx,45h (`+=`) ; push 0 ; push 3Eh (`+`) ; call find_overload
const ADD_CASE: &str = "B9 45 00 00 00 6A 00 6A 3E E8";
/// Its end, after its `call release` of the old value (which `argload` retargets, so not part of the patterns):
/// add esp,10h ; then the write: mov ecx,[state] ; movzx eax,word [ebp-4C8h] (the opcode's position) ;
/// mov edi,[ebp-4D8h] ; mov [ecx+14h],ax
const ADD_WRITE: &str = "83 C4 10 8B 0D ?? ?? ?? ?? 0F B7 85 ?? ?? ?? ?? 8B BD ?? ?? ?? ?? 66 89 41 14";
const ADD_POS: usize = 12;
/// The interpreter loop's head, which saves the opcode's position twice: movzx eax,word [esi+14h] ;
/// mov edx,[esi+10h] ; mov [ebp-4C8h],ax ; mov [ebp-4B8h],eax ; shl eax,2
const LOOP: &str = "0F B7 46 14 8B 56 10 66 89 85 ?? ?? ?? ?? 89 85 ?? ?? ?? ?? C1 E0 02";
const LOOP_POS16: usize = 10;
const LOOP_POS32: usize = 16;
/// `<<=` and `>>=`: mov ecx,4Dh (`<<=`) ; push 0 ; push 82h (`<<`) ; call find_overload ; mov esi,eax ;
/// add esp,10h ; cmp eax,0FFFFh ; then je no_overload or jne overload_case. Each has its own case.
const SHL: &str = "B9 4D 00 00 00 6A 00 68 82 00 00 00 E8 ?? ?? ?? ?? 89 C6 83 C4 10 3D FF FF 00 00";
const SHR: &str = "B9 4E 00 00 00 6A 00 68 83 00 00 00 E8 ?? ?? ?? ?? 89 C6 83 C4 10 3D FF FF 00 00";
/// The case's end, after its `call release` of the old value: mov eax,[ebp-4B8h] ; mov esi,[state] ; add esp,10h ;
/// mov [ebp-4E8h],eax ; jmp next. From the first `mov` on, the release's arguments are still pushed, as at `+=`'s
/// write.
const CASE_END: &str = "8B 85 ?? ?? ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 10 89 85 ?? ?? ?? ?? E9";
const CASE_POS: usize = 2;

/// The function the `call` that ends right before `at` called as shipped, not relied on.
fn call_before(core: &Image, at: usize) -> Result<usize> {
    let b = core.haystack(at - 5, 5)?;
    ensure!(b[0] == 0xE8, "{at:#x}: no call before");
    Ok(at.wrapping_add(i32::from_le_bytes([b[1], b[2], b[3], b[4]]) as usize))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let add = core.find_unique(ADD_CASE, "+= overload case")?;
    let write = core.find_unique_in(add, 0x100, ADD_WRITE, "+= overload case: write")?;
    let release = call_before(core, write)?;
    let pos16 = core.u32_at(write + ADD_POS)?;
    let head = core.find_unique(LOOP, "interpreter loop")?;
    ensure!(core.u32_at(head + LOOP_POS16)? == pos16, "+= overload case: not the loop's opcode position");
    let pos32 = core.u32_at(head + LOOP_POS32)?;
    let mut sites = vec![];
    for (ida, what) in [(SHL, "<<= opcode"), (SHR, ">>= opcode")] {
        let op = core.find_unique(ida, what)?;
        let b = core.branch_at(op + ida.split_whitespace().count())?;
        let case = match b.cond {
            Some(Cond::Equal) => b.next,
            Some(Cond::NotEqual) => b.target,
            _ => bail!("{what}: no overload case"),
        };
        let end = core.find_unique_in(case, 0x120, CASE_END, what)?;
        ensure!(
            call_before(core, end)? == release && core.u32_at(end + CASE_POS)? == pos32,
            "{what}: overload case not as expected"
        );
        let stub = Stub::new()
            .bytes_u32(&[0x8B, 0x85], pos32, &[]) // mov eax,[ebp+pos32]
            .bytes_u32(&[0x66, 0x89, 0x85], pos16, &[]) // mov [ebp+pos16],ax
            .jmp(write) // jmp += write
            .exec()?;
        core.jump(end, 6, stub)?;
        sites.push(end);
    }
    Ok(core.report(&sites))
}
