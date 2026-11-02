//! `hubsock`, Linux: three stubs, in Register's existing-entry branch, its mode-0 test, and the connection's close
//! (connection in ebx, its connecting socket at +530h).

use super::{FOUND, Found, keeps_pending, owns_pending};
use crate::base::code::{branch, exec_code, jcc};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// OpenClientSocket's registration of the pending connect: `mov [eax+4],data ; mov [eax],callback ; pop edx ;
/// pop ecx ; push eax (the {callback, data} pair) ; push CONNECT_DONE ; push name ; push 8 (mode) ; push ebx (socket) ;
/// push this ; call Register`.
const OPEN: &str =
    "89 ?? 04 89 ?? 5A 59 50 68 ?? ?? ?? ?? FF B4 24 ?? 00 00 00 6A 08 53 FF B4 24 ?? 00 00 00 E8 ?? ?? ?? ??";
const CONNECT_DONE: usize = 9;
const REGISTER_CALL: usize = 31;
/// Register's existing-entry branch: `movzx eax,byte [esp+4] ; mov [esi+10h],al ; mov eax,[esp+8] ;
/// mov [esi+4],eax ; mov eax,[esp+0Ch] ; mov [esi+0Ch],eax` (mode, callback, data), then its epilogue.
const OVERWRITE: &str = "0F B6 44 24 04 88 46 10 8B 44 24 08 89 46 04 8B 44 24 0C 89 46 0C 83 C4 1C 5B 5E 5F 5D C3";
const EPILOGUE: usize = 22;
/// Register's mode-0 test: `test al,al ; jne search ; cmp ebx,-1 ; jge unregister`, then the search of the entries
/// (edi = this, ebx = socket): `mov edx,[edi+COUNT] ; mov ecx,[edi+ENTRIES] ; xor eax,eax ; test edx,edx ; jle ;
/// (padding) ; L: mov esi,[ecx+eax*4] ; test esi,esi ; je ; cmp ebx,[esi] (socket) ; je`.
const MODE0: &str = "84 C0 75 09 83 FB FF 0F 8D ?? ?? ?? ?? 8B 57 ?? 8B 4F ?? 31 C0 85 D2 0F 8E ?? ?? ?? ?? \
                     8D 74 26 00 90 8B 34 81 85 F6 74 ?? 3B 1E 0F 84";
/// Its `cmp ebx,-1 ; jge unregister` (replaced), the `jge` field, and the table offsets.
const MODE0_CMP: usize = 4;
const MODE0_LEN: usize = 9;
const COUNT: usize = 15;
const ENTRIES: usize = 18;
/// The connection's close: `mov eax,[ebx+8] ; cmp eax,-1 ; jl chain ; cmp eax,-1 ; jne closemain ;` then
/// `mov eax,[ebx+530h] ; cmp eax,-1 ; je skip ; sub esp,8 ; push eax ; push socklib ; call CloseSocket ;
/// add esp,10h ; mov [ebx+530h],-1`, then skip.
const CLOSE: &str = "8B 43 08 83 F8 FF 0F 8C ?? ?? ?? ?? 83 F8 FF 0F 85 ?? ?? ?? ?? 8B 83 30 05 00 00 83 F8 FF 74 ?? \
                     83 EC 08 50 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 C7 83 30 05 00 00 FF FF FF FF";
/// Its `cmp eax,-1 ; je skip` (replaced) and the socket library.
const CLOSE_CMP: usize = 27;
const SOCKLIB: usize = 37;
const CLOSE_LEN: usize = 59;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let open = core.find_unique(OPEN, "pending connect registration")?;
    let connect_done = core.u32_at(open + CONNECT_DONE)?;
    let register = core.rel32(open + REGISTER_CALL)?;
    let overwrite = core.find_unique(OVERWRITE, "Register: existing entry")?;
    let mode0 = core.find_unique(MODE0, "Register: mode 0")?;
    if !(register..register + 0x200).contains(&overwrite) || !(register..register + 0x40).contains(&mode0) {
        bail!("Register sites are not in the function OpenClientSocket calls");
    }
    let epilogue = overwrite + EPILOGUE;
    let unregister = core.rel32(mode0 + MODE0_CMP + 5)?;
    let after_mode0 = mode0 + MODE0_CMP + MODE0_LEN;
    let [count] = core.array(mode0 + COUNT)?;
    let [entries] = core.array(mode0 + ENTRIES)?;

    let close = core.find_unique(CLOSE, "clisock close")?;
    let cmp = close + CLOSE_CMP;
    let skip = core.rel8(cmp + 4)?;
    let after = cmp + 5;
    if skip != close + CLOSE_LEN {
        bail!("clisock close: unexpected skip target");
    }
    let cb = connect_done.to_le_bytes();

    // Register, existing entry (esi): don't turn a pending connect into anything but a connect.
    let stub_overwrite = exec_code(34, |s| {
        let mut v = vec![0x81, 0x7E, 0x04]; // cmp dword [esi+4],CB
        v.extend(cb);
        v.extend([0x75, 0x0F]); // jne orig
        v.extend([0x81, 0x7C, 0x24, 0x08]); // cmp dword [esp+8],CB (the new callback)
        v.extend(cb);
        v.extend([0x74, 0x05]); // je orig
        v.extend(branch(0xE9, s + v.len(), epilogue, 5)); // jmp epilogue
        v.extend([0x0F, 0xB6, 0x44, 0x24, 0x04]); // orig: movzx eax,byte [esp+4]
        v.extend(branch(0xE9, s + v.len(), overwrite + 5, 5)); // jmp back
        v
    })?;
    // Register, mode 0 (edi = this, ebx = socket): keep a pending connect's entry. The stack is 16-byte aligned here.
    let stub_mode0 = exec_code(36, |s| {
        let mut v = vec![0x83, 0xFB, 0xFF]; // cmp ebx,-1
        v.extend([0x7C, 0x1A]); // jl cont
        v.extend([0x83, 0xEC, 0x08]); // sub esp,8
        v.push(0x53); // push ebx
        v.push(0x57); // push edi
        v.extend(branch(0xE8, s + v.len(), keeps_pending as unsafe extern "C" fn(usize, i32) -> bool as usize, 5));
        v.extend([0x83, 0xC4, 0x10]); // add esp,10h
        v.extend([0x84, 0xC0]); // test al,al
        v.extend(jcc(0x85, s + v.len(), epilogue)); // jne epilogue
        v.extend(branch(0xE9, s + v.len(), unregister, 5)); // jmp unregister
        v.extend(branch(0xE9, s + v.len(), after_mode0, 5)); // cont: jmp search
        v
    })?;
    // The connection's close (ebx = connection, eax = its connecting socket): close it only if the pending connect
    // is ours, else forget the number. The stack is 16-byte aligned here.
    let stub_close = exec_code(47, |s| {
        let mut v = vec![0x83, 0xF8, 0xFF]; // cmp eax,-1
        v.extend(jcc(0x84, s + v.len(), skip)); // je skip
        v.push(0x50); // push eax
        v.extend([0x83, 0xEC, 0x04]); // sub esp,4
        v.push(0x53); // push ebx
        v.push(0x50); // push eax
        v.extend(branch(0xE8, s + v.len(), owns_pending as unsafe extern "C" fn(i32, usize) -> bool as usize, 5));
        v.extend([0x83, 0xC4, 0x0C]); // add esp,0Ch
        v.extend([0x84, 0xC0]); // test al,al
        v.push(0x58); // pop eax
        v.extend(jcc(0x85, s + v.len(), after)); // jne close
        v.extend([0xC7, 0x83, 0x30, 0x05, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]); // mov dword [ebx+530h],-1
        v.extend(branch(0xE9, s + v.len(), skip, 5)); // jmp skip
        v
    })?;

    core.jump(overwrite, 5, stub_overwrite)?;
    core.jump(mode0 + MODE0_CMP, MODE0_LEN, stub_mode0)?;
    core.jump(cmp, 5, stub_close)?;
    let socklib = core.ptr_at(close + SOCKLIB)?;
    FOUND.commit(cx, Found { connect_done, socklib, entries: usize::from(entries), count: usize::from(count) });
    Ok(format!("at +{:#x}", register - core.base))
}
