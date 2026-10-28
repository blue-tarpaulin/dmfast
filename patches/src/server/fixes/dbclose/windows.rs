//! `dbclose`, Windows: `Open()` calls `Close()`, so `Close()`'s `call [free]` is the only free to patch. `Open()` is
//! reordered in place.

use super::{ARRAY, CAPACITY};
use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // Close(), after detaching the queries: push [esi+18h] ; call [free] ; add esp,4 ; mov ecx,esi ;
    // call close_handle ; push 0 ; lea ecx,[esi+14h] ; call DMString::operator= ; pop esi ; ret.
    // 516.1670 clears +18h and +20h after the free, so it doesn't match.
    let at = core.find_unique(
        "FF 76 18 FF 15 ?? ?? ?? ?? 83 C4 04 8B CE E8 ?? ?? ?? ?? 6A 00 8D 4E 14 E8 ?? ?? ?? ?? 5E C3",
        "database close",
    )?;
    // Open(name), after the SEH prologue: mov edi,ecx ; push [ebp+8] ; call DMString::operator= (name) ;
    // cmp dword [edi+4],0 ; je +7 ; mov ecx,edi ; call Close ; cmp [edi+8],edi ; je +7 ; mov ecx,edi ; call
    let open = core.find_unique(
        "8B F9 FF 75 08 E8 ?? ?? ?? ?? 83 7F 04 00 74 07 8B CF E8 ?? ?? ?? ?? 39 7F 08 74 07 8B CF E8",
        "database open",
    )?;
    let slot = core.iat_slot("free")?;
    if core.ptr_at(at + 5)? != slot {
        bail!("database close: doesn't call free");
    }
    let assign = core.rel32(open + 6)?;
    let close = core.rel32(open + 19)?;
    // Open's Close() holds the free: push esi ; mov esi,ecx ; cmp dword [esi+1Ch],0
    if core.rel32(at + 25)? != assign
        || !(at - 0x3F..at).contains(&close)
        || core.bytes(close, 7)? != [0x56, 0x8B, 0xF1, 0x83, 0x7E, 0x1C, 0x00]
    {
        bail!("database open: doesn't call the close above");
    }
    let stub = exec_code(20, |_| {
        let mut v = vec![0xC7, 0x46, ARRAY, 0, 0, 0, 0]; // mov dword [esi+18h],0
        v.extend([0xC7, 0x46, CAPACITY, 0, 0, 0, 0]); // mov dword [esi+20h],0
        v.extend([0xFF, 0x25]); // jmp [free]
        v.extend((slot as u32).to_le_bytes());
        v
    })?;
    // Open+2, the same 21 bytes in 516.1670's order: cmp dword [edi+4],0 ; je +5 ; call Close (ecx is still edi) ;
    // push [ebp+8] ; mov ecx,edi ; call DMString::operator=
    let mut code = vec![0x83, 0x7F, 0x04, 0x00, 0x74, 0x05];
    code.extend(branch(0xE8, open + 8, close, 5));
    code.extend([0xFF, 0x75, 0x08, 0x8B, 0xCF]);
    code.extend(branch(0xE8, open + 18, assign, 5));
    core.call_iat_to(at + 3, slot, stub)?;
    core.write_code(open + 2, &code)?;
    Ok(format!("at {:#x}, {:#x}", at - core.base, open - core.base))
}
