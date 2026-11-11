//! `iconchain`, Linux: the conversion to a resource runs [`super::prepare`] first, its store calls [`push`], and the
//! server icon context's look-up and save slots go to [`get`] and [`save_icon`].

use std::arch::naked_asm;

use super::{FOUND, Found, loaded, prepare, save, stored};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// The list's push of a resource load (`01 <id> <byte>`), cdecl (the list, the id, the byte).
type Push = unsafe extern "C" fn(usize, u32, u32);
/// The server icon context's look-up of a loaded icon (the context, the resource id) and save (and the icon).
type Get = unsafe extern "C" fn(usize, u32) -> usize;
type Save = unsafe extern "C" fn(usize, u32, usize) -> i32;

/// The conversion of an icon record to a resource (cdecl: the result, popped by it, this, the record), from its
/// moved entry.
static ORIGINAL: Foreign<unsafe extern "C" fn()> = Foreign::new();
static PUSH: Foreign<Push> = Foreign::new();
static GET: Foreign<Get> = Foreign::new();

/// In the conversion: mov eax,[ebp+10h] (the record) ; lea ebx,[eax+8] (its list) ; mov [esp],ebx ;
/// call single_load ; add esp,10h ; cmp eax,0FFFFh ; jne (one load: return its resource) ; mov eax,[ebp+10h] ;
/// mov edi,[eax+0Ch] ; test edi,edi ; jne ; push ebx ; call is_flat ; add esp,10h ; test al,al ; jne pixels
const CONVERT: &str = "8B 45 10 8D 58 08 89 1C 24 E8 ?? ?? ?? ?? 83 C4 10 3D FF FF 00 00 0F 85 ?? ?? ?? ?? 8B 45 10 8B \
                       78 0C 85 FF 0F 85 ?? ?? ?? ?? 83 EC 0C 53 E8 ?? ?? ?? ?? 83 C4 10 84 C0 0F 85";
/// From the conversion's entry: push ebp ; mov ebp,esp ; push edi ; push esi ; lea eax,[ebp-..] ; push ebx ;
/// sub esp,.. ; push eax ; call
const ENTRY_IDA: &str = "55 89 E5 57 56 8D 85 ?? ?? ?? ?? 53 81 EC ?? ?? ?? ?? 50 E8";
const ENTRY_AT: usize = 0x4C;
/// push ebp ; mov ebp,esp ; push edi ; push esi: 5 position-independent bytes.
const ENTRY: usize = 5;
/// Its test of a list too long to store: mov eax,[ebp+10h] ; cmp dword [eax+0Ch],100h ; ja pixels
const TOO_LONG: &str = "8B 45 10 81 78 0C 00 01 00 00 0F 87";
/// Its pixel path: sub esp,0Ch ; push [ebp+10h] ; call render ; mov [esp],ebx ; call flatten ; mov eax,[ebp+10h] ;
/// mov ecx,[ebp+10h] ; mov byte [ebp-..],3
const PIXELS: &str = "83 EC 0C FF 75 10 E8 ?? ?? ?? ?? 89 1C 24 E8 ?? ?? ?? ?? 8B 45 10 8B 4D 10 C6 85 ?? ?? ?? ?? 03";
/// Its store: call clear ; add esp,0Ch ; movzx eax,byte [ebp-..] ; push 0Ch ; push the resource ; call id_of ;
/// add esp,0Ch ; push byte ; push eax ; push ebx ; call push ; mov ebx,[ebp+10h]
const STORE: &str = "E8 ?? ?? ?? ?? 83 C4 0C 0F B6 85 ?? ?? ?? ?? 6A 0C FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 89 85 ?? \
                     ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 0C FF B5 ?? ?? ?? ?? 50 53 E8 ?? ?? ?? ?? 8B 5D 10";
const STORE_PUSH: usize = 51;
/// The push: grow the buffer by 2, write 01 and a terminator, grow by 5, write the id.
const PUSH_IDA: &str = "57 56 53 8B 5C 24 10 8B 74 24 18 8B 4B 04 8B 7B 08 8B 03 8D 51 02";
/// The flatten (cdecl, the list): grow the buffer to at least 2 bytes, then length = 1, "?\0" (as `iconwrap`).
const FLATTEN_IDA: &str = "56 53 83 EC 04 8B 5C 24 10 8B 53 08 8B 03 83 FA 02 7F ?? B9 02 00 00 00 89 CE 29 D6 83 C2 32 \
                           83 FE 32 0F 43 D1 83 EC 08 89 53 08 52 50 E8 ?? ?? ?? ?? 83 C4 10 89 03 C7 43 04 01 00 00 \
                           00 C6 00 3F 8B 03 C6 40 01 00 83 C4 04 5B 5E C3";
/// In the render function, making its context: mov dword [esp],2Ch ; mov dword [ebp-24h],ServerIconContext's
/// vtable
const VTABLE: &str = "C7 04 24 2C 00 00 00 C7 45 DC";
/// The vtable's look-up (mov eax,[esp+8] ; mov [esp+4],eax ; jmp: the id to the cache's look-up) and save slots.
const GET_SLOT: usize = 8;
const GET_IDA: &str = "8B 44 24 08 89 44 24 04 E9";
const SAVE_SLOT: usize = 0x14;

/// BYOND's walk of an operation list to its next resource load: (saves) ; mov esi,[esp+20h] (the cursor) ;
/// mov ebx,[esi] ; movzx eax,byte [ebx] ; test al,al ; je end ; (padding) ; cmp al,4Eh ; ja ; jmp [table]
const WALK_IDA: &str = "57 56 53 83 EC 10 8B 74 24 20 8B 1E 0F B6 03 84 C0 74 ?? 8D 74 26 00 90 3C 4E 77 ?? 0F B6 D0 FF \
                        24 95";

/// The conversion's entry: [`prepare`] with the record (esp 16-byte aligned at the call), then the conversion with
/// everything as it was.
#[unsafe(naked)]
unsafe extern "C" fn convert_stub() {
    naked_asm!(
        "sub esp, 8",
        "push dword ptr [esp + 0x14]", // the record
        "call {prepare}",
        "add esp, 12",
        "jmp dword ptr [{orig}]",
        prepare = sym prepare,
        orig = sym ORIGINAL,
    )
}

/// The store's push of the new resource into the record's list (+8): [`stored`], then the push.
unsafe extern "C" fn push(list: usize, id: u32, byte: u32) {
    stored(list - 8, id);
    // SAFETY: BYOND's call, passed on.
    unsafe { PUSH.get()(list, id, byte) }
}

/// The server icon context's look-up: the fix's copy while it draws, else BYOND's.
unsafe extern "C" fn get(context: usize, id: u32) -> usize {
    // SAFETY: BYOND's call, passed on.
    loaded(id).unwrap_or_else(|| unsafe { GET.get()(context, id) })
}

/// The server icon context's save (stock: returns 0).
unsafe extern "C" fn save_icon(_context: usize, id: u32, icon: usize) -> i32 {
    // SAFETY: BYOND's call: the icon it loaded resource `id` into.
    unsafe { save(id, icon) };
    0
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let at = core.find_unique(CONVERT, "icon to resource conversion")?;
    let f = at - ENTRY_AT;
    core.require(f, ENTRY_IDA, "icon to resource conversion: entry")?;
    core.find_unique_in(f, 0x200, TOO_LONG, "icon to resource conversion: length test")?;
    let pixels = core.find_unique_in(f, 0x300, PIXELS, "icon to resource conversion: pixel path")?;
    let render = core.call_target(pixels + 6)?;
    let flatten = core.call_target(pixels + 14)?;
    core.require(flatten, FLATTEN_IDA, "icon to resource conversion: flatten")?;
    let store = core.find_unique_in(f, 0x700, STORE, "icon to resource conversion: store")?;
    let push_fn = core.follow_call(store + STORE_PUSH, PUSH_IDA, "icon to resource conversion: push")?;
    ensure!(pixels < store, "icon to resource conversion: store before the pixel path");
    let vtable = core.ptr_at(core.find_unique_in(render, 0x100, VTABLE, "icon render: context vtable")? + 10)?;
    let get_fn = core.ptr_at(vtable + GET_SLOT)?;
    core.require(get_fn, GET_IDA, "server icon context: look-up")?;
    let base_save = core.export(c"_ZN11IconContext4SaveEmP6DMIcon")?;
    ensure!(core.ptr_at(vtable + SAVE_SLOT)? == base_save, "server icon context: saves loaded icons");
    FOUND.commit(
        cx,
        Found {
            render: Foreign::at(render),
            flatten: Foreign::at(flatten),
            copy: Foreign::at(core.export(c"_ZN6DMIconC1ERKS_")?),
            destroy: Foreign::at(core.export(c"_ZN6DMIconD1Ev")?),
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
