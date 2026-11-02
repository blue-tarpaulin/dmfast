//! `hubsock`, Windows: three stubs, in Register's existing-entry branch, its mode-0 test, and the connection's close
//! (connection in esi, its connecting socket at +528h).

use super::{FOUND, Found, keeps_pending, owns_pending};
use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

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
    ensure!(
        (register..register + 0x200).contains(&overwrite) && (register..register + 0x40).contains(&mode0),
        "Register sites are not in the function OpenClientSocket calls"
    );
    let count = core.u8_at(mode0 + COUNT)?;
    let entries = core.u8_at(mode0 + ENTRIES)?;

    // The close function is the one the connect timer's callback calls (other copies of CLOSE are candidates).
    let close_fn = core.call_target(core.find_unique(TIMER, "connect timer callback")? + 2)?;
    let close = core.probe_unique(CLOSE, "clisock close", |c| (close_fn..close_fn + CLOSE_SPAN).contains(&c))?;
    let cmp = close + CLOSE_CMP;
    let skip = core.rel8(cmp + 4)?;
    let after = cmp + 5;
    ensure!(skip == close + CLOSE_LEN, "clisock close: unexpected skip target");

    // Register, existing entry (ecx): don't turn a pending connect into anything but a connect.
    let mut s = Stub::new();
    let orig = s.label();
    let stub_overwrite = s
        .bytes_u32(&[0x81, 0x79, 0x04], connect_done, &[]) // cmp dword [ecx+4],CB
        .jcc8(Cond::NotEqual, orig) // jne orig
        .bytes_u32(&[0x81, 0x7D, 0x14], connect_done, &[]) // cmp dword [ebp+14h],CB (the new callback)
        .jcc8(Cond::Equal, orig) // je orig
        .bytes(&[0x5F]) // pop edi
        .bytes(&[0x5E]) // pop esi
        .bytes(&[0x5B]) // pop ebx
        .bytes(&[0x5D]) // pop ebp
        .bytes(&[0xC2, 0x14, 0x00]) // ret 14h
        .bind(orig)
        .bytes(core.bytes(overwrite, OVERWRITE_LEN)?) // orig: mov al,[ebp+0Ch] ; mov [ecx+10h],al
        .jmp(overwrite + OVERWRITE_LEN) // jmp back
        .exec()?;
    // Register, mode 0 (ebx = this, esi = socket): keep a pending connect's entry.
    let mut s = Stub::new();
    let unregister = s.label();
    let stub_mode0 = s
        .bytes(&[0x83, 0xFE, 0xFE]) // cmp esi,-2
        .jcc(Cond::LessOrEqual, mode0 + SEARCH) // jle search
        .bytes(&[0x56]) // push esi
        .bytes(&[0x53]) // push ebx
        .call(keeps_pending as unsafe extern "C" fn(usize, i32) -> bool) // call keeps_pending
        .bytes(&[0x83, 0xC4, 0x08]) // add esp,8
        .bytes(&[0x84, 0xC0]) // test al,al
        .jcc8(Cond::Equal, unregister) // je unregister
        .bytes(&[0x5E]) // pop esi
        .bytes(&[0x5B]) // pop ebx
        .bytes(&[0x5D]) // pop ebp
        .bytes(&[0xC2, 0x14, 0x00]) // ret 14h
        .bind(unregister)
        .bytes(&[0x8B, 0xCB]) // unregister: mov ecx,ebx
        .jmp(mode0 + UNREGISTER) // jmp Unregister's call
        .exec()?;
    // The connection's close (esi = connection, eax = its connecting socket): close it only if the pending connect
    // is ours, else forget the number.
    let stub_close = Stub::new()
        .bytes(&[0x83, 0xF8, 0xFF]) // cmp eax,-1
        .jcc(Cond::Equal, skip) // je skip
        .bytes(&[0x50]) // push eax
        .bytes(&[0x56]) // push esi
        .bytes(&[0x50]) // push eax
        .call(owns_pending as unsafe extern "C" fn(i32, usize) -> bool) // call owns_pending
        .bytes(&[0x83, 0xC4, 0x08]) // add esp,8
        .bytes(&[0x84, 0xC0]) // test al,al
        .bytes(&[0x58]) // pop eax
        .jcc(Cond::NotEqual, after) // jne close
        .bytes(&[0xC7, 0x86, 0x28, 0x05, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]) // mov dword [esi+528h],-1
        .jmp(skip) // jmp skip
        .exec()?;

    core.jump(overwrite, OVERWRITE_LEN, stub_overwrite)?;
    core.jump(mode0 + MODE0_CMP, 5, stub_mode0)?;
    core.jump(cmp, 5, stub_close)?;
    let socklib = core.ptr_at(close + SOCKLIB)?;
    FOUND.commit(cx, Found { connect_done, socklib, entries: usize::from(entries), count: usize::from(count) });
    Ok(core.report(&[register]))
}
