//! `varread`, Windows: byondcore's user var read (cdecl, value in EDX:EAX) and its .dmb load hooked at entry.

use std::arch::naked_asm;

use super::cache::{Found, Kind, STATE, State, Value, flush, read};
use crate::base::code::{Foreign, hx, trampoline};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail};

/// The read: own vars (binary search), then type, initial-value search, declaration search (static: the statics
/// table, else the undefined-variable error). Offsets below are into this.
const READ: &str = "55 8B EC 83 EC 08 53 8B 5D 0C 56 8B 75 08 57 53 56 E8 ?? ?? ?? ?? 8B 7D 10 83 C4 08 81 E7 FF FF FF 00 \
    85 C0 74 ?? 8B 18 33 C9 0F B7 40 04 89 5D 10 85 C0 74 ?? 8D 34 08 D1 EE 8B D6 03 D2 3B 7C D3 04 74 ?? 8B 5D 10 \
    8D 56 01 0F 42 C6 0F 47 CA 3B C8 72 ?? 8B 75 08 8B 5D 0C 53 56 E8 ?? ?? ?? ?? 6A 09 52 50 E8 ?? ?? ?? ?? 8D 4D F8 \
    51 57 50 E8 ?? ?? ?? ?? 83 C4 20 84 C0 74 ?? 8B 45 F8 8B 55 FC 5F 5E 5B 8B E5 5D C3 8B 44 D3 08 8B 54 D3 0C 5F 5E \
    5B 8B E5 5D C3 53 56 E8 ?? ?? ?? ?? 6A 09 52 50 E8 ?? ?? ?? ?? 8D 4D 13 51 8D 4D FC 51 57 50 E8 ?? ?? ?? ?? 83 C4 24 \
    84 C0 74 ?? F6 45 13 01 74 ?? 8B 0D ?? ?? ?? ?? 8B 55 FC 5F 5E 5B 8B 04 D1 8B 54 D1 04 8B E5 5D C3";
/// `push ebp ; mov ebp,esp ; sub esp,8 ; push ebx`: position-independent, moved for the hook.
const ENTRY: usize = 7;

/// Own-var-list lookup: switch on the tag (`dec eax ; cmp eax,max ; ja ; movzx eax,byte [map+eax] ;
/// jmp [cases+eax*4]`).
const OWN: &str =
    "55 8B EC 8B 4D 08 0F B6 C1 48 56 83 F8 ?? 0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ??";
/// Value -> type: its first step reads the object's type field as a value, with the same switch shape.
const TYPE_PATH: &str = "55 8B EC 53 56 FF 75 0C FF 75 08 E8";
const TYPE_FIELD: &str =
    "55 8B EC 8B 4D 08 0F B6 C1 48 83 F8 ?? 0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85 ?? ?? ?? ??";
/// `id_of(value, tag)`: the id if the value has that tag, else 0FFFFh.
const ID_OF: &str = "55 8B EC 8A 4D 08 B8 FF FF 00 00 3A 4D 10 0F 44 45 0C 5D C3";
/// An object table lookup: `id < [count] ? [table][id] : 0`.
const GETTER: &str = "55 8B EC 8B 4D 08 3B 0D ?? ?? ?? ?? 72 04 33 C0 5D C3 A1 ?? ?? ?? ?? 8B 04 88 5D C3";
/// The initial-value search's start and the offsets of the type table's count and pointer and the id-array table in
/// it; 516.1686 added a check for type 0FFFFh.
const FIND_INIT: [(&str, [usize; 3]); 2] = [
    (
        "55 8B EC 51 8B 45 08 53 56 57 3B 05 ?? ?? ?? ?? 0F 83 ?? ?? ?? ?? 3D FF FF 00 00 0F 84 ?? ?? ?? ?? \
         69 C0 ?? ?? ?? ?? 03 05 ?? ?? ?? ?? 0F 84 ?? ?? ?? ?? 8B 48 78 3B 0D ?? ?? ?? ?? 73 ?? A1",
        [0xC, 0x29, 0x3F],
    ),
    (
        "55 8B EC 51 8B 45 08 53 56 57 3B 05 ?? ?? ?? ?? 0F 83 ?? ?? ?? ?? 69 C0 ?? ?? ?? ?? 03 05 ?? ?? ?? ?? \
         74 ?? 8B 48 78 3B 0D ?? ?? ?? ?? 73 ?? A1",
        [0xC, 0x1E, 0x30],
    ),
];
/// The .dmb load: the function with this prologue that stores the id-array table (`mov [table],eax` within
/// [`LOAD_SPAN`]). Its first 5 bytes (`push ebp ; mov ebp,esp ; push -1`) are moved; the relocated `push imm32` after
/// them stays.
const LOAD_FN: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC ?? 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
    64 A3 00 00 00 00 8B 7D 08 57 E8 ?? ?? ?? ?? 8B 1D ?? ?? ?? ?? 8B F0 56 89 35 ?? ?? ?? ?? FF D3 83 C4 08 85 F6 74 08 \
    85 C0 0F 84";
const LOAD_SPAN: usize = 0x1000;
const LOAD_ENTRY: usize = 5;

/// Value tags read inline: datum, obj, mob, area.
const TAGS: [u8; 4] = [0x21, 0x02, 0x03, 0x04];

/// The original read (its moved entry, then the rest) and the original .dmb load, for the stubs.
static ORIGINAL: Foreign<unsafe extern "C" fn(u32, u32, u32) -> u64> = Foreign::new();
static LOAD: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// BYOND's functions the cache is filled from (cdecl; a value comes back in EDX:EAX).
pub(super) struct Calls {
    own_list: Foreign<unsafe extern "C" fn(u32, u32) -> usize>,
    type_path: Foreign<unsafe extern "C" fn(u32, u32) -> u64>,
    id_of: Foreign<unsafe extern "C" fn(u32, u32, u32) -> u32>,
    find_init: Foreign<unsafe extern "C" fn(u32, u32, *mut Value) -> u8>,
    find_decl: Foreign<unsafe extern "C" fn(u32, u32, *mut u32, *mut u8) -> u8>,
}

fn value(v: u64) -> Value {
    Value { tag: v as u32, data: (v >> 32) as u32 }
}

/// The original read.
pub(super) unsafe fn original(tag: u32, id: u32, name: u32) -> Value {
    value(unsafe { ORIGINAL.get()(tag, id, name) })
}

impl Calls {
    /// The own-var list of a value, or 0.
    pub(super) unsafe fn own_list(&self, tag: u32, id: u32) -> usize {
        unsafe { self.own_list.get()(tag, id) }
    }

    /// The type id of a value, as the read computes it.
    pub(super) unsafe fn type_id(&self, tag: u32, id: u32) -> u32 {
        unsafe {
            let v = value(self.type_path.get()(tag, id));
            self.id_of.get()(v.tag, v.data, 9)
        }
    }

    /// Type `t`'s initial value of `name`.
    pub(super) unsafe fn find_init(&self, t: u32, name: u32) -> Option<Value> {
        let mut v = Value::default();
        (unsafe { self.find_init.get()(t, name, &mut v) } != 0).then_some(v)
    }

    /// The statics-table index of type `t`'s static var `name`.
    pub(super) unsafe fn find_static(&self, t: u32, name: u32) -> Option<u32> {
        let (mut index, mut flags) = (0u32, 0u8);
        let found = unsafe { self.find_decl.get()(t, name, &mut index, &mut flags) } != 0;
        (found && flags & 1 != 0).then_some(index)
    }
}

/// The read's entry (`tag, id, name`): [`read`] answers into a local returned in EDX:EAX, else the original runs with
/// the untouched arguments (its own errors and exceptions, no frame of ours on the stack).
#[unsafe(naked)]
unsafe extern "C" fn read_stub() {
    naked_asm!(
        "sub esp, 8",
        "push esp",                    // out: the local
        "push dword ptr [esp + 0x18]", // name
        "push dword ptr [esp + 0x18]", // id
        "push dword ptr [esp + 0x18]", // tag
        "call {read}",
        "add esp, 16",
        "test al, al",
        "jz 2f",
        "pop eax",
        "pop edx",
        "ret",
        "2:",
        "add esp, 8",
        "jmp dword ptr [{orig}]",
        read = sym read,
        orig = sym ORIGINAL,
    )
}

/// The .dmb load's entry: [`flush`], then the load.
#[unsafe(naked)]
unsafe extern "C" fn load_stub() {
    naked_asm!(
        "pushad",
        "call {flush}",
        "popad",
        "jmp dword ptr [{load}]",
        flush = sym flush,
        load = sym LOAD,
    )
}

/// The instruction after a short (`op8 rel8`) or near (`0F op+10h rel32`) conditional jump at `a`.
fn after_jcc(core: &Image, a: usize, short: u8) -> Result<usize> {
    match core.array::<2>(a)? {
        [b, _] if b == short => Ok(a + 2),
        [0x0F, b] if b == short + 0x10 => Ok(a + 6),
        _ => bail!("expected a conditional jump at {a:#x}"),
    }
}

/// Case address of `tag` in a `dec eax ; cmp eax,max ; ... movzx eax,byte [map+eax] ; jmp [cases+eax*4]` switch
/// at `f`, its `max`, `map`, and `cases` operands at the `ops` offsets.
fn case(core: &Image, f: usize, ops: [usize; 3], tag: u8) -> Result<usize> {
    let i = tag as usize - 1;
    if i > core.array::<1>(f + ops[0])?[0] as usize {
        bail!("no case for value tag {tag:#x}");
    }
    let [slot] = core.array(core.ptr_at(f + ops[1])? + i)?;
    core.ptr_at(core.ptr_at(f + ops[2])? + 4 * slot as usize)
}

/// An own-var-list case: `mov ecx,[ebp+0Ch] ; cmp ecx,[count] ; jae null ; mov eax,[table] ; mov eax,[eax+ecx*4] ;
/// test eax,eax ; jz null ; add eax,own ; pop esi ; pop ebp ; ret`. (count, table, offset).
fn own_case(core: &Image, a: usize) -> Result<(usize, usize, usize)> {
    if !core.is_at(a, "8B 4D 0C 3B 0D")? {
        bail!("own var list case: not as expected");
    }
    let count = core.ptr_at(a + 5)?;
    let a = after_jcc(core, a + 9, 0x73)?;
    if !core.is_at(a, "A1 ?? ?? ?? ?? 8B 04 88 85 C0")? {
        bail!("own var list case: not as expected");
    }
    let table = core.ptr_at(a + 1)?;
    let a = after_jcc(core, a + 10, 0x74)?;
    if !core.is_at(a, "83 C0 ?? 5E 5D C3")? {
        bail!("own var list case: not as expected");
    }
    Ok((count, table, core.array::<1>(a + 2)?[0] as usize))
}

/// A type-field case: the object from a table getter (from `[ebp+0Ch]`, or for datums and areas through
/// `id_of(value, tag)`), null: no type, else a value of the field at `[eax+key]`. (getter, key).
fn type_case(core: &Image, a: usize, tag: u8, id_of: usize) -> Result<(usize, usize)> {
    let (getter, a) = if core.is_at(a, "FF 75 0C E8 ?? ?? ?? ?? 83 C4 04 85 C0")? {
        (core.rel32(a + 4)?, a + 13)
    } else if core.is_at(a, "6A ?? FF 75 0C 51 E8 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? 83 C4 10 85 C0")?
        && core.array::<1>(a + 1)? == [tag]
        && core.rel32(a + 7)? == id_of
    {
        (core.rel32(a + 13)?, a + 22)
    } else {
        bail!("type field case: not as expected");
    };
    let a = after_jcc(core, a, 0x74)?;
    if !core.is_at(a, "6A")? {
        bail!("type field case: not as expected");
    }
    let key = match core.array::<2>(a + 2)? {
        [0xFF, 0x30] => 0,
        [0xFF, 0x70] => core.array::<1>(a + 4)?[0] as usize,
        [0xFF, 0xB0] => core.ptr_at(a + 4)?,
        _ => bail!("type field case: not as expected"),
    };
    Ok((getter, key))
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(READ, "user var read")?;
    let own = core.rel32(f + 0x12)?;
    let type_path = core.rel32(f + 0x5D)?;
    let id_of = core.rel32(f + 0x66)?;
    let init = core.rel32(f + 0x71)?;
    let decl = core.rel32(f + 0xB3)?;
    if core.rel32(f + 0x9B)? != type_path || core.rel32(f + 0xA4)? != id_of {
        bail!("user var read: the two type lookups differ");
    }
    let statics = core.ptr_at(f + 0xC6)?;
    for (at, pat, what) in [(id_of, ID_OF, "value id"), (own, OWN, "own var list"), (type_path, TYPE_PATH, "type path")]
    {
        if !core.is_at(at, pat)? {
            bail!("{what}: not as expected");
        }
    }
    let field = core.rel32(type_path + 12)?;
    if !core.is_at(field, TYPE_FIELD)? {
        bail!("type field: not as expected");
    }
    let mut offsets = None;
    for (pat, o) in FIND_INIT {
        if core.is_at(init, pat)? {
            offsets = Some(o);
            break;
        }
    }
    let Some([nprotos, protos, ids]) = offsets else { bail!("initial value search: not as expected") };
    let (nprotos, protos, ids) = (core.ptr_at(init + nprotos)?, core.ptr_at(init + protos)?, core.ptr_at(init + ids)?);

    let mut kinds = [None; 256];
    for tag in TAGS {
        let (count, table, own_off) = own_case(core, case(core, own, [0xD, 0x17, 0x1E], tag)?)?;
        let (getter, key) = type_case(core, case(core, field, [0xC, 0x16, 0x1D], tag)?, tag, id_of)?;
        // The type's object is the own vars' object.
        if !core.is_at(getter, GETTER)? || core.ptr_at(getter + 8)? != count || core.ptr_at(getter + 19)? != table {
            bail!("type field (value tag {tag:#x}): object from another table");
        }
        kinds[tag as usize] = Some(Kind { count, table, own: own_off, key });
    }

    // The .dmb load: the function that allocates the id-array table.
    let store = format!("A3 {}", hx(ids));
    let mut loads = Vec::new();
    for l in core.probe_all(LOAD_FN)? {
        if !core.find_in(l, LOAD_SPAN, &store)?.is_empty() {
            loads.push(l);
        }
    }
    let [load] = loads[..] else { bail!(".dmb load: expected 1 match, found {}", loads.len()) };
    core.rely(load, LOAD_FN.split_whitespace().count())?;

    core.untouched(&[(f, ENTRY), (load, LOAD_ENTRY)])?;
    ORIGINAL.set(trampoline(core.bytes(f, ENTRY)?, f + ENTRY)?);
    LOAD.set(trampoline(core.bytes(load, LOAD_ENTRY)?, load + LOAD_ENTRY)?);
    STATE.set(State::new(Found {
        kinds,
        protos,
        nprotos,
        statics,
        calls: Calls {
            own_list: Foreign::at(own),
            type_path: Foreign::at(type_path),
            id_of: Foreign::at(id_of),
            find_init: Foreign::at(init),
            find_decl: Foreign::at(decl),
        },
    }))?;
    core.jump(load, LOAD_ENTRY, load_stub as unsafe extern "C" fn())?;
    core.jump(f, ENTRY, read_stub as unsafe extern "C" fn())?;
    Ok(format!("at +{:#x}, load at +{:#x}", f - core.base, load - core.base))
}
