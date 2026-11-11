//! `iconchain`, Windows: the conversion to a resource runs [`super::prepare`] first, its store calls [`push`], and the
//! server icon context's look-up and save slots go to [`get`] and [`save_icon`].

use std::arch::naked_asm;

use super::{FOUND, Found, loaded, prepare, save, stored};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The list's push of a resource load (`01 <id> <byte>`), thiscall (the list, the id, the byte).
type Push = unsafe extern "thiscall" fn(usize, u32, u32);
/// The server icon context's look-up of a loaded icon (the context, the resource id) and save (and the icon).
type Get = unsafe extern "thiscall" fn(usize, u32) -> usize;
type Save = unsafe extern "thiscall" fn(usize, u32, usize) -> i32;

/// The conversion of an icon record to a resource (thiscall: this, the result, the record), from its moved entry.
static ORIGINAL: Foreign<unsafe extern "C" fn()> = Foreign::new();
static PUSH: Foreign<Push> = Foreign::new();
static GET: Foreign<Get> = Foreign::new();

/// In the conversion (edi = the record, ebx = its list): lea ebx,[edi+8] ; mov ecx,ebx ; call single_load ;
/// cmp eax,0FFFFh ; je ; (one load: return its resource) ; cmp dword [edi+0Ch],0 ; je pixels ; mov ecx,ebx ;
/// call is_flat ; test al,al ; jne pixels ; cmp dword [edi+0Ch],100h ; ja pixels
const CONVERT: &str = "8D 5F 08 8B CB E8 ?? ?? ?? ?? 3D FF FF 00 00 74 ?? 6A 0C 8B CB E8 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? \
                       8B 75 08 83 C4 08 89 06 89 56 04 E9 ?? ?? ?? ?? 83 7F 0C 00 74 ?? 8B CB E8 ?? ?? ?? ?? 84 C0 75 \
                       ?? 81 7F 0C 00 01 00 00 77";
/// From the conversion's entry: push ebp ; mov ebp,esp ; push -1 ; push handler ; (SEH frame) ; mov esi,ecx
const ENTRY_IDA: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 81 EC D8 00 00 00 53 56 57 A1 ?? ?? ?? ?? \
                         33 C5 50 8D 45 F4 64 A3 00 00 00 00 8B F1";
const ENTRY_AT: usize = 0x88;
/// push ebp ; mov ebp,esp ; push -1: 5 position-independent bytes.
const ENTRY: usize = 5;
/// Its pixel path: mov ecx,edi ; call render ; mov ecx,ebx ; call flatten ; mov ax,[edi+0Ch] ; mov [edi+14h],ax ;
/// mov byte [ebp-20h],3
const PIXELS: &str = "8B CF E8 ?? ?? ?? ?? 8B CB E8 ?? ?? ?? ?? 66 8B 47 0C 66 89 47 14 C6 45 E0 03";
/// Its store: mov [edi],esi ; mov [edi+4],eax (the record's resource) ; call clear ; push byte ; push 0Ch ;
/// push [ebp-1Ch] ; push esi ; call id_of ; add esp,0Ch ; mov ecx,ebx ; push eax ; call push ; cmp dword [edi+18h],0
const STORE: &str = "8B 45 E4 8B CB 89 37 89 47 04 E8 ?? ?? ?? ?? FF 75 D1 6A 0C FF 75 E4 56 E8 ?? ?? ?? ?? 83 C4 0C \
                     8B CB 50 E8 ?? ?? ?? ?? 83 7F 18 00";
const STORE_PUSH: usize = 35;
/// The push: grow the buffer by 2, write 01 and a terminator, grow by 5, write the id.
const PUSH_IDA: &str = "55 8B EC 53 56 8B F1 8B 4E 04 57 8B 5E 08 8D 7E 04 8D 51 02";
/// The flatten (fastcall, the list): grow the buffer to at least 2 bytes, then length = 1, "?\0" (as `iconwrap`).
const FLATTEN_IDA: &str = "56 8B F1 BA 02 00 00 00 8B 4E 08 3B CA 7F ?? 8B C2 2B C1 83 F8 32 73 03 8D 51 32 52 FF 36 89 \
                           56 08 FF 15 ?? ?? ?? ?? 83 C4 08 89 06 8B 06 C7 46 04 01 00 00 00 C6 00 3F 8B 06 5E C6 40 \
                           01 00 C3";
/// In the render function: lea ecx,[ecx+300h] ; call context ; push eax ; push [esi+18h]
const CONTEXT_CALL: &str = "8D 89 00 03 00 00 E8 ?? ?? ?? ?? 50 FF 76 18";
/// In the context getter: push 2Ch ; mov dword [ebp-1Ch],ServerIconContext's vtable
const VTABLE: &str = "6A 2C C7 45 E4";
/// The vtable's look-up (push ebp ; mov ebp,esp ; push esi ; push 0Ch ; push [id] ; call) and save slots.
const GET_SLOT: usize = 8;
const GET_IDA: &str = "55 8B EC 56 6A 0C FF 75 08 E8";
const SAVE_SLOT: usize = 0x14;

/// BYOND's walk of an operation list to its next resource load: push ebp ; mov ebp,esp ; (saves) ;
/// mov edi,[ebp+8] (the cursor) ; mov esi,[edi] ; mov dl,[esi] ; test dl,dl ; je end ; mov ebx,14h
const WALK_IDA: &str = "55 8B EC 53 56 57 8B 7D 08 8B 37 8A 16 84 D2 0F 84 ?? ?? ?? ?? BB 14 00 00 00";

/// The conversion's entry: [`prepare`] with the record, then the conversion with everything as it was.
#[unsafe(naked)]
unsafe extern "C" fn convert_stub() {
    naked_asm!(
        "push ecx",
        "push dword ptr [esp + 0x0C]", // the record
        "call {prepare}",
        "add esp, 4",
        "pop ecx",
        "jmp dword ptr [{orig}]",
        prepare = sym prepare,
        orig = sym ORIGINAL,
    )
}

/// The store's push of the new resource into the record's list (+8): [`stored`], then the push.
unsafe extern "thiscall" fn push(list: usize, id: u32, byte: u32) {
    stored(list - 8, id);
    // SAFETY: BYOND's call, passed on.
    unsafe { PUSH.get()(list, id, byte) }
}

/// The server icon context's look-up: the fix's copy while it draws, else BYOND's.
unsafe extern "thiscall" fn get(context: usize, id: u32) -> usize {
    // SAFETY: BYOND's call, passed on.
    loaded(id).unwrap_or_else(|| unsafe { GET.get()(context, id) })
}

/// The server icon context's save (stock: returns 0).
unsafe extern "thiscall" fn save_icon(_context: usize, id: u32, icon: usize) -> i32 {
    // SAFETY: BYOND's call: the icon it loaded resource `id` into.
    unsafe { save(id, icon) };
    0
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(CONVERT, "icon to resource conversion")?;
    let f = at - ENTRY_AT;
    core.require(f, ENTRY_IDA, "icon to resource conversion: entry")?;
    let pixels = core.find_unique_in(f, 0x200, PIXELS, "icon to resource conversion: pixel path")?;
    let render = core.call_target(pixels + 2)?;
    let flatten = core.follow_call(pixels + 9, FLATTEN_IDA, "icon to resource conversion: flatten")?;
    let store = core.find_unique_in(f, 0x600, STORE, "icon to resource conversion: store")?;
    let push_fn = core.follow_call(store + STORE_PUSH, PUSH_IDA, "icon to resource conversion: push")?;
    ensure!(pixels < store, "icon to resource conversion: store before the pixel path");
    let context = core.call_target(core.find_unique_in(render, 0x100, CONTEXT_CALL, "icon render: context")? + 6)?;
    let vtable = core.ptr_at(core.find_unique_in(context, 0x80, VTABLE, "icon render: context vtable")? + 5)?;
    let get_fn = core.ptr_at(vtable + GET_SLOT)?;
    core.require(get_fn, GET_IDA, "server icon context: look-up")?;
    let base_save = core.export(c"?Save@IconContext@@UAEHKPAUDMIcon@@@Z")?;
    ensure!(core.ptr_at(vtable + SAVE_SLOT)? == base_save, "server icon context: saves loaded icons");
    FOUND.commit(
        cx,
        Found {
            render: Foreign::at(render),
            flatten: Foreign::at(flatten),
            copy: Foreign::at(core.export(c"??0DMIcon@@QAE@ABU0@@Z")?),
            destroy: Foreign::at(core.export(c"??1DMIcon@@QAE@XZ")?),
            walk: Foreign::at(core.find_unique(WALK_IDA, "icon operation walker")?),
        },
    );
    PUSH.set(push_fn);
    GET.set(get_fn);
    core.call(store + STORE_PUSH, 5, push as Push)?;
    core.write_data(vtable + GET_SLOT, &(get as Get as usize as u32).to_le_bytes())?;
    core.write_data(vtable + SAVE_SLOT, &(save_icon as Save as usize as u32).to_le_bytes())?;
    ORIGINAL.set(core.hook(f, ENTRY, convert_stub as unsafe extern "C" fn())?);
    Ok(core.report(&[f]))
}
