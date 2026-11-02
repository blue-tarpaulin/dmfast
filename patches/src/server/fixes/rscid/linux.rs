//! `rscid`, Linux: the get-or-add's call of the dynamic set's insert (cdecl; the only one) calls [`insert`].

use std::ffi::c_void;

use super::{Key, next_is_null};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, bail};

type Insert = unsafe extern "C" fn(usize, *const Key) -> *mut c_void;
type Method = unsafe extern "C" fn(*mut Key);

/// What the 0xFFFF branch uses: the insert, the key's constructor, its vtable after construction and before
/// destruction (the base class's), the destructor, and the set's entry count offset.
struct Found {
    insert: Foreign<Insert>,
    ctor: Foreign<Method>,
    vtable: u32,
    base_vtable: u32,
    dtor: Foreign<Method>,
    count: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// The get-or-add: push the dynamic set and key ; call insert ; mov esi,eax ; test eax,eax ; je ... ;
/// mov eax,[ebx+318h] ; add eax,[esi+18h] ; cmp eax,0FFFFh ; jne ... ; then the 0FFFFh branch: construct the
/// sentinel key (vtable, length 0), move the entry, insert the sentinel (length -1), insert the real key, destroy
/// the sentinel (base vtable)
const GET_OR_ADD: &str = "83 EC 08 8D 83 F0 02 00 00 FF 75 0C 50 E8 ?? ?? ?? ?? 83 C4 10 89 C6 85 C0 0F 84 ?? ?? ?? ?? 8B \
                          83 18 03 00 00 03 46 18 3D FF FF 00 00 75 ?? 83 EC 0C 8D 7D C0 57 E8 ?? ?? ?? ?? B8 00 FF FF FF \
                          5A 59 66 89 45 E4 8D 83 C8 02 00 00 56 89 C6 50 C7 45 C0 ?? ?? ?? ?? C7 45 E0 00 00 00 00 E8 ?? \
                          ?? ?? ?? 59 58 57 56 C7 45 E0 FF FF FF FF C6 45 E4 FF E8 ?? ?? ?? ?? 58 5A FF 75 0C 56 E8 ?? ?? \
                          ?? ?? 89 C6 89 3C 24 C7 45 C0 ?? ?? ?? ?? E8 ?? ?? ?? ??";
/// Its `call insert`; in the branch, the constructor's call field, the vtable, the two inserts' call fields, the base
/// vtable, and the destructor's call field.
const CALL: usize = 13;
const CTOR: usize = 55;
const VTABLE: usize = 83;
const INSERTS: [usize; 2] = [115, 126];
const BASE_VTABLE: usize = 138;
const DTOR: usize = 143;
/// The set's pool allocator: shr edx,cl ; shl edx,3 ; add dword [esi+count],1 ; add dword [eax+edx+4],1 ;
/// mov eax,[ebx+8] ; mov [esi+0Ch],eax
const ALLOC: &str = "D3 EA C1 E2 03 83 46 ?? 01 83 44 10 04 01 8B 43 08 89 46 0C";
const COUNT: usize = 7;

/// The dynamic set's insert, after inserting the sentinel if `key` would get 0xFFFF.
///
/// # Safety
/// BYOND's call: `set` is the dynamic set and `key` a live key.
unsafe extern "C" fn insert(set: usize, key: *const Key) -> *mut c_void {
    let f = FOUND.get();
    // SAFETY: the dynamic set (caller); BYOND's functions, called as its 0xFFFF branch calls them.
    unsafe {
        if next_is_null(set, f.count) {
            let mut sentinel = Key::EMPTY;
            f.ctor.get()(&mut sentinel);
            sentinel.make_sentinel(f.vtable);
            f.insert.get()(set, &sentinel);
            sentinel.vtable = f.base_vtable;
            f.dtor.get()(&mut sentinel);
        }
        f.insert.get()(set, key)
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let s = core.find_unique(GET_OR_ADD, "rsc id get-or-add insert")?;
    let insert_fn = core.rel32(s + CALL + 1)?;
    for field in INSERTS {
        if core.rel32(s + field)? != insert_fn {
            bail!("the 0xFFFF branch inserts with another routine");
        }
    }
    let count = core.u8_at(core.find_unique(ALLOC, "rsc set entry count")? + COUNT)?;
    FOUND.commit(
        cx,
        Found {
            insert: Foreign::at(insert_fn),
            ctor: Foreign::at(core.rel32(s + CTOR)?),
            vtable: core.u32_at(s + VTABLE)?,
            base_vtable: core.u32_at(s + BASE_VTABLE)?,
            dtor: Foreign::at(core.rel32(s + DTOR)?),
            count: usize::from(count),
        },
    );
    core.call(s + CALL, 5, insert as Insert)?;
    Ok(format!("insert at +{:#x}", insert_fn - core.base))
}
