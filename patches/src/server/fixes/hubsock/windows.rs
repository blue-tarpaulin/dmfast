//! `hubsock`, Windows: three stubs, in Register's existing-entry branch, its mode-0 test, and the connection's close
//! (connection in esi, its connecting socket at +528h).

use super::{FOUND, Found, keeps_pending, owns_pending};
use crate::base::code::{branch, exec_code, jcc};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// OpenClientSocket's registration of the pending connect: `mov ecx,[ebp+10h] (done callback) ; push eax (the
/// {callback, data} pair) ; push CONNECT_DONE ; push [ebp+0Ch] (name) ; mov [eax],ecx ; mov ecx,edi ; push 8
/// (mode) ; push esi (socket) ; mov [eax+4],ebx (data) ; call Register`.
const OPEN: &str = "8B 4D 10 50 68 ?? ?? ?? ?? FF 75 0C 89 08 8B CF 6A 08 56 89 58 04 E8 ?? ?? ?? ??";
const CONNECT_DONE: usize = 5;
const REGISTER_CALL: usize = 23;
/// Register's existing-entry branch (ecx = the entry; mode, callback, data) with its epilogue.
const OVERWRITE: &str = "8A 45 0C 88 41 10 8B 45 14 5F 89 41 04 8B 45 18 5E 89 41 0C 5B 5D C2 14 00";
/// Its first two instructions, replaced: mov al,[ebp+0Ch] ; mov [ecx+10h],al.
const OVERWRITE_LEN: usize = 6;
/// Register's start: `cmp byte [ebp+0Ch],0 ; push ebx ; push esi ; mov esi,[ebp+8] ; mov ebx,ecx ; jne search ;
/// cmp esi,-2 ; jle search ; push esi ; call Unregister ; pop esi ; pop ebx ; pop ebp ; ret 14h`, then the search
/// of the entries: `push edi ; mov edi,[ebx+COUNT] ; xor eax,eax ; test edi,edi ; jle ; mov edx,[ebx+ENTRIES] ;
/// nop ; L: mov ecx,[edx] ; test ecx,ecx ; je ; cmp [ecx],esi (socket) ; je`.
const MODE0: &str = "80 7D 0C 00 53 56 8B 75 08 8B D9 75 11 83 FE FE 7E 0C 56 E8 ?? ?? ?? ?? 5E 5B 5D C2 14 00 \
                     57 8B 7B ?? 33 C0 85 FF 7E ?? 8B 53 ?? 66 90 8B 0A 85 C9 74 ?? 39 31 74 ??";
/// Its `cmp esi,-2 ; jle search` (replaced), the unregister (`push esi ; call Unregister`), the search, and the
/// table offsets.
const MODE0_CMP: usize = 13;
const UNREGISTER: usize = 18;
const SEARCH: usize = 30;
const COUNT: usize = 33;
const ENTRIES: usize = 42;
/// The connection's close function: the main socket (+8) closes, then `mov eax,[esi+528h] ; cmp eax,-1 ; je skip ;
/// push eax ; mov ecx,socklib ; call CloseSocket ; mov [esi+528h],-1`, then skip.
const CLOSE: &str = "83 F8 FF 74 12 50 B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? C7 46 08 FF FF FF FF 8B 86 28 05 00 00 83 F8 FF \
                     74 15 50 B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? C7 86 28 05 00 00 FF FF FF FF";
/// Its `cmp eax,-1 ; je skip` (replaced) and the socket library.
const CLOSE_CMP: usize = 29;
const SOCKLIB: usize = 36;
const CLOSE_LEN: usize = 55;
/// How far into the close function the close is.
const CLOSE_SPAN: usize = 0x200;
/// The connect timer's callback: `push 3 ; call close ; ret`.
const TIMER: &str = "6A 03 E8 ?? ?? ?? ?? C3";

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
    let [count] = core.array(mode0 + COUNT)?;
    let [entries] = core.array(mode0 + ENTRIES)?;

    // The close function is the one the connect timer's callback calls (other copies of CLOSE are candidates).
    let close_fn = core.rel32(core.find_unique(TIMER, "connect timer callback")? + 3)?;
    let close = core.probe_unique(CLOSE, "clisock close", |c| (close_fn..close_fn + CLOSE_SPAN).contains(&c))?;
    let cmp = close + CLOSE_CMP;
    let skip = core.rel8(cmp + 4)?;
    let after = cmp + 5;
    if skip != close + CLOSE_LEN {
        bail!("clisock close: unexpected skip target");
    }
    let cb = connect_done.to_le_bytes();

    // Register, existing entry (ecx): don't turn a pending connect into anything but a connect.
    let replaced = core.bytes(overwrite, OVERWRITE_LEN)?.to_vec();
    let stub_overwrite = exec_code(36, |s| {
        let mut v = vec![0x81, 0x79, 0x04]; // cmp dword [ecx+4],CB
        v.extend(cb);
        v.extend([0x75, 0x10]); // jne orig
        v.extend([0x81, 0x7D, 0x14]); // cmp dword [ebp+14h],CB (the new callback)
        v.extend(cb);
        v.extend([0x74, 0x07]); // je orig
        v.extend([0x5F, 0x5E, 0x5B, 0x5D]); // pop edi ; pop esi ; pop ebx ; pop ebp
        v.extend([0xC2, 0x14, 0x00]); // ret 14h
        v.extend(&replaced); // orig: mov al,[ebp+0Ch] ; mov [ecx+10h],al
        v.extend(branch(0xE9, s + v.len(), overwrite + OVERWRITE_LEN, 5)); // jmp back
        v
    })?;
    // Register, mode 0 (ebx = this, esi = socket): keep a pending connect's entry.
    let stub_mode0 = exec_code(36, |s| {
        let mut v = vec![0x83, 0xFE, 0xFE]; // cmp esi,-2
        v.extend(jcc(0x8E, s + v.len(), mode0 + SEARCH)); // jle search
        v.push(0x56); // push esi
        v.push(0x53); // push ebx
        v.extend(branch(0xE8, s + v.len(), keeps_pending as unsafe extern "C" fn(usize, i32) -> bool as usize, 5));
        v.extend([0x83, 0xC4, 0x08]); // add esp,8
        v.extend([0x84, 0xC0]); // test al,al
        v.extend([0x74, 0x06]); // je unregister
        v.extend([0x5E, 0x5B, 0x5D]); // pop esi ; pop ebx ; pop ebp
        v.extend([0xC2, 0x14, 0x00]); // ret 14h
        v.extend([0x8B, 0xCB]); // unregister: mov ecx,ebx
        v.extend(branch(0xE9, s + v.len(), mode0 + UNREGISTER, 5)); // jmp Unregister's call
        v
    })?;
    // The connection's close (esi = connection, eax = its connecting socket): close it only if the pending connect
    // is ours, else forget the number.
    let stub_close = exec_code(44, |s| {
        let mut v = vec![0x83, 0xF8, 0xFF]; // cmp eax,-1
        v.extend(jcc(0x84, s + v.len(), skip)); // je skip
        v.push(0x50); // push eax
        v.push(0x56); // push esi
        v.push(0x50); // push eax
        v.extend(branch(0xE8, s + v.len(), owns_pending as unsafe extern "C" fn(i32, usize) -> bool as usize, 5));
        v.extend([0x83, 0xC4, 0x08]); // add esp,8
        v.extend([0x84, 0xC0]); // test al,al
        v.push(0x58); // pop eax
        v.extend(jcc(0x85, s + v.len(), after)); // jne close
        v.extend([0xC7, 0x86, 0x28, 0x05, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]); // mov dword [esi+528h],-1
        v.extend(branch(0xE9, s + v.len(), skip, 5)); // jmp skip
        v
    })?;

    core.jump(overwrite, OVERWRITE_LEN, stub_overwrite)?;
    core.jump(mode0 + MODE0_CMP, 5, stub_mode0)?;
    core.jump(cmp, 5, stub_close)?;
    let socklib = core.ptr_at(close + SOCKLIB)?;
    FOUND.commit(cx, Found { connect_done, socklib, entries: usize::from(entries), count: usize::from(count) });
    Ok(format!("at +{:#x}", register - core.base))
}
