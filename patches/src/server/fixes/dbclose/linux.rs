//! `dbclose`, Linux: `Close()` and the copy of it inlined in `Open()` both free the array, so both frees are patched.
//! The name assignment moves to `Open()`'s tail, after the close.

use super::{ARRAY, CAPACITY};
use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // Close(), after detaching the queries: push [ebx+18h] ; call free ; mov [esp],ebx ; add ebx,14h ;
    // call close_handle. 516.1670 clears +18h and +20h after the free, so it doesn't match.
    let close = core.find_unique("FF 73 18 E8 ?? ?? ?? ?? 89 1C 24 83 C3 14 E8", "database close")?;
    // Open(db, name), whole function:
    //   +00 push esi ; push ebx ; sub esp,0Ch ; mov ebx,[esp+18h] ; mov esi,[esp+1Ch]
    //   +0D push esi ; push ebx ; call DMString::operator=        (db->name = name)
    //   +14 mov eax,[ebx+4] ; add esp,10h ; test eax,eax ; jne +30 ; jmp +5A
    //   +20 detach loop ; +37 sub esp,0Ch ; +3A push [ebx+18h] ; +3D call free ; mov [esp],ebx ;
    //   +45 call close_handle ; pop eax ; lea eax,[ebx+14h] ; pop edx ; push 0 ; push eax ;
    //   +52 call DMString::operator= ; add esp,10h
    //   +5A mov [esp+14h],esi ; mov [esp+10h],ebx ; add esp,4 ; pop ebx ; pop esi ; jmp open_code
    let open = core.find_unique(
        "56 53 83 EC 0C 8B 5C 24 18 8B 74 24 1C 56 53 E8 ?? ?? ?? ?? 8B 43 04 83 C4 10 85 C0 75 12 EB 3A \
         8B 43 18 83 EC 0C FF 30 E8 ?? ?? ?? ?? 83 C4 10 8B 4B 1C 85 C9 75 E9 83 EC 0C FF 73 18 E8 ?? ?? ?? ?? \
         89 1C 24 E8 ?? ?? ?? ?? 58 8D 43 14 5A 6A 00 50 E8 ?? ?? ?? ?? 83 C4 10 \
         89 74 24 14 89 5C 24 10 83 C4 04 5B 5E E9",
        "database open",
    )?;
    let sites = [close + 3, open + 0x3D];
    let free = core.rel32(sites[0] + 1)?;
    let frees = core.import_calls("free", free)?;
    if !sites.iter().all(|s| frees.contains(s)) {
        bail!("database close: doesn't call free");
    }
    let assign = core.rel32(open + 0x10)?;
    if core.rel32(open + 0x53)? != assign {
        bail!("database open: name not set with DMString::operator=");
    }
    let clear = exec_code(19, |s| {
        let mut v = vec![0xC7, 0x43, ARRAY, 0, 0, 0, 0]; // mov dword [ebx+18h],0
        v.extend([0xC7, 0x43, CAPACITY, 0, 0, 0, 0]); // mov dword [ebx+20h],0
        v.extend(branch(0xE9, s + v.len(), free, 5)); // jmp free
        v
    })?;
    // Open's tail, now after the close.
    let tail = exec_code(26, |s| {
        let mut v = vec![0x83, 0xEC, 0x08, 0x56, 0x53]; // sub esp,8 ; push esi ; push ebx
        v.extend(branch(0xE8, s + v.len(), assign, 5)); // call DMString::operator=
        v.extend([0x83, 0xC4, 0x10]); // add esp,10h
        v.extend([0x89, 0x74, 0x24, 0x14, 0x89, 0x5C, 0x24, 0x10]); // mov [esp+14h],esi ; mov [esp+10h],ebx
        v.extend(branch(0xE9, s + v.len(), open + 0x62, 5)); // jmp back
        v
    })?;
    for s in sites {
        core.call(s, 5, clear)?;
    }
    // The early assignment becomes sub esp,8, keeping the add esp,10h after it balanced.
    core.write_code(open + 0x0D, &[0x83, 0xEC, 0x08, 0x90, 0x90, 0x90, 0x90])?;
    core.jump(open + 0x5A, 8, tail)?;
    Ok(format!("at +{:#x}, +{:#x}", close - core.base, open - core.base))
}
