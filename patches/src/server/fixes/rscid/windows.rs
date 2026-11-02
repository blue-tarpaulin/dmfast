//! `rscid`, Windows: both get-or-add functions' call of the dynamic set's insert (thiscall) calls [`insert`].

use std::ffi::c_void;

use super::{Key, next_is_null};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::{Result, ensure};

type Insert = unsafe extern "thiscall" fn(usize, *const Key) -> *mut c_void;
type Method = unsafe extern "thiscall" fn(*mut Key);

/// What the 0xFFFF branch uses: the insert, the key's constructor, vtable, and destructor, and the set's entry count
/// offset.
struct Found {
    insert: Foreign<Insert>,
    ctor: Foreign<Method>,
    vtable: u32,
    dtor: Foreign<Method>,
    count: usize,
}

static FOUND: Installed<Found> = Installed::new();

/// The get-or-add: push edi ; lea ecx,[esi+2F0h] ; call insert ; mov ebx,eax ; test ebx,ebx ; je ... ;
/// mov eax,[ebx+18h]
const GET_OR_ADD: &str = "57 8D 8E F0 02 00 00 E8 ?? ?? ?? ?? 8B D8 85 DB 0F 84 ?? ?? ?? ?? 8B 43 18";
/// Its 0xFFFF branch: construct the sentinel at [ebp-34h] (vtable, [+20h] = 0, word [+24h] = 0FF00h), move the entry
/// to the main set, set [+20h] = -1 and byte [+24h] = 0FFh, insert the sentinel, insert the real key, destroy the
/// sentinel, reload the dynamic set's id base
const BRANCH: &str = "3D FF FF 00 00 75 ?? 8D 4D CC E8 ?? ?? ?? ?? C7 45 CC ?? ?? ?? ?? C7 45 EC 00 00 00 00 66 C7 45 F0 \
                      00 FF 53 8D 8E C8 02 00 00 C7 45 FC 00 00 00 00 E8 ?? ?? ?? ?? 8D 45 CC C7 45 EC FF FF FF FF 50 \
                      8D 8E C8 02 00 00 C6 45 F0 FF E8 ?? ?? ?? ?? 57 8D 8E C8 02 00 00 E8 ?? ?? ?? ?? 8D 4D CC 8B D8 \
                      E8 ?? ?? ?? ?? 8B 8E 18 03 00 00 8B 43 18 03 C1";
/// The variant get-or-add: push eax ; lea ecx,[ebx+2F0h] ; call insert ; mov edi,eax ; test edi,edi ; jne ... ;
/// mov edi,0FFFFh
const VARIANT: &str = "50 8D 8B F0 02 00 00 E8 ?? ?? ?? ?? 8B F8 85 FF 75 0A BF FF FF 00 00";
/// And its 0xFFFF branch, the same sentinel at [ebp-5Ch].
const VARIANT_BRANCH: &str = "3D FF FF 00 00 75 ?? 8D 4D A4 E8 ?? ?? ?? ?? C7 45 A4 ?? ?? ?? ?? C7 45 C4 00 00 00 00 66 \
                              C7 45 C8 00 FF 57 8D 8B C8 02 00 00 C6 45 FC 01 E8 ?? ?? ?? ?? 8D 45 A4 C7 45 C4 FF FF FF \
                              FF 50 8D 8B C8 02 00 00 C6 45 C8 FF E8 ?? ?? ?? ?? 8D 45 CC 50 8D 8B C8 02 00 00 E8 ?? ?? \
                              ?? ?? 8D 4D A4 8B F8 E8";
/// The get-or-adds' `call insert`.
const CALL: usize = 7;
/// In the 0xFFFF branches: the constructor's call field, the vtable, and the destructor's call field; the two
/// inserts' call fields (the sentinel's and the real key's) differ.
const CTOR: usize = 11;
const VTABLE: usize = 18;
const DTOR: usize = 98;
const INSERTS: [usize; 2] = [76, 88];
const VARIANT_INSERTS: [usize; 2] = [73, 88];
/// The set's pool allocator: mov [edi+0Ch],esi ; mov eax,[edi+18h] ; inc dword [edi+count] ; inc [eax+edx*8+4] ;
/// mov eax,[esi+8] ; mov [edi+0Ch],eax
const ALLOC: &str = "89 77 0C 8B 47 18 FF 47 ?? FF 44 D0 04 8B 46 08 89 47 0C";
const COUNT: usize = 8;

/// The dynamic set's insert, after inserting the sentinel if `key` would get 0xFFFF.
///
/// # Safety
/// BYOND's call: `set` is the dynamic set and `key` a live key.
unsafe extern "thiscall" fn insert(set: usize, key: *const Key) -> *mut c_void {
    let f = FOUND.get();
    // SAFETY: the dynamic set (caller); BYOND's functions, called as its 0xFFFF branch calls them.
    unsafe {
        if next_is_null(set, f.count) {
            let mut sentinel = Key::EMPTY;
            f.ctor.get()(&mut sentinel);
            sentinel.make_sentinel(f.vtable);
            f.insert.get()(set, &sentinel);
            f.dtor.get()(&mut sentinel);
        }
        f.insert.get()(set, key)
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let a = core.find_unique(GET_OR_ADD, "rsc id get-or-add insert")?;
    let branch_a = core.find_unique(BRANCH, "rsc id 0xFFFF branch")?;
    let b = core.find_unique(VARIANT, "rsc id variant insert")?;
    let branch_b = core.find_unique(VARIANT_BRANCH, "rsc id variant 0xFFFF branch")?;
    let insert_fn = core.call_target(a + CALL)?;
    ensure!(core.call_target(b + CALL)? == insert_fn, "the two get-or-add functions insert with different routines");
    let inserts = INSERTS.map(|o| branch_a + o).into_iter().chain(VARIANT_INSERTS.map(|o| branch_b + o));
    for field in inserts {
        ensure!(core.rel32(field)? == insert_fn, "a 0xFFFF branch inserts with another routine");
    }
    let (ctor, vtable, dtor) =
        (core.rel32(branch_a + CTOR)?, core.u32_at(branch_a + VTABLE)?, core.rel32(branch_a + DTOR)?);
    ensure!(
        core.rel32(branch_b + CTOR)? == ctor
            && core.u32_at(branch_b + VTABLE)? == vtable
            && core.rel32(branch_b + DTOR)? == dtor,
        "the two 0xFFFF branches build different sentinel keys"
    );
    let count = core.u8_at(core.find_unique(ALLOC, "rsc set entry count")? + COUNT)?;
    let count = usize::from(count);
    let found =
        Found { insert: Foreign::at(insert_fn), ctor: Foreign::at(ctor), vtable, dtor: Foreign::at(dtor), count };
    core.call(a + CALL, 5, insert as Insert)?;
    core.call(b + CALL, 5, insert as Insert)?;
    FOUND.commit(cx, found);
    Ok(format!("insert at +{:#x}", insert_fn - core.base))
}
