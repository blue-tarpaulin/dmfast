//! `finalpath`, Windows: the parent-type pass's call goes through [`parent_pass`], the override check's lookup call
//! through [`lookup`].

use super::{FOUND, Found, Ids, Layout, Lookup, lookup, unwrap_finals};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::names::Names;

/// Before the definition passes: push "Assigning parent types\n" ; call log ; add esp,4 ; push [root] ;
/// call pass ; mov ecx,[root] ; add esp,4 ; xor ebx,ebx ; movzx edi,word [ecx+22h]
const PASS: &str = "68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 04 FF 35 ?? ?? ?? ?? E8 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 83 C4 04 33 DB 0F B7 79 22";
const PASS_LOG: usize = 1;
const PASS_CALL: usize = 19;
/// The parent-type pass (cdecl: node): push ebp ; mov ebp,esp ; push edi ; mov edi,[ebp+8] ; test edi,edi ; je
const PASS_FN: &str = "55 8B EC 57 8B 7D 08 85 FF 0F 84";
/// The override check: lookup(&node, &flags, &builtin) ; if not built in and found: is the node in a `proc` or
/// `verb` block (push <proc> ; call ancestor ; push <verb> ; call ancestor) ; if not, test the flags for final (200000h).
const SITE: &str = "8B 7D 08 8D 45 0B 50 8D 45 F8 89 7D FC 50 8D 45 FC C6 45 0B 00 50 E8 ?? ?? ?? ?? 83 C4 0C 80 7D 0B \
                    00 0F 85 ?? ?? ?? ?? 8B CF 84 C0 0F 84 ?? ?? ?? ?? 68 ?? ?? 00 00 E8 ?? ?? ?? ?? 85 C0 75 ?? 68 ?? \
                    ?? 00 00 8B CF E8 ?? ?? ?? ?? 85 C0 75 ?? F7 45 F8 00 00 20 00";
const SITE_CALL: usize = 22;
const SITE_PROC: usize = 51;
const SITE_VERB: usize = 65;
/// Then, the definition found must be defined (flags bit 0) or the check is tried again in the next pass:
/// mov eax,[ebp-4] ; test eax,eax ; je ; test byte [eax+FLAGS],1 ; je ; push [eax+0Ch] ; call
const DEFINED: &str = "8B 45 FC 85 C0 0F 84 ?? ?? ?? ?? F6 40 ?? 01 0F 84 ?? ?? ?? ?? FF 70 0C E8";
const DEFINED_FLAGS: usize = 13;
/// The lookup (cdecl): push ebp ; mov ebp,esp ; push esi ; mov esi,[ebp+8] ; mov edx,[esi] ; mov eax,edx ; test eax,eax
const LOOKUP: &str = "55 8B EC 56 8B 75 08 8B 16 8B C2 85 C0 74";
/// A node's parent type (fastcall): its last `..` child's type, else its parent node. Matched after its first 5 bytes
/// (movzx edx,word [ecx+count] ; push esi), which the parent cache may hook: push edi ; test edx,edx ; je ;
/// mov edi,[ecx+CHILDREN] ; ... ; cmp dword [eax],45h ; ... ; mov eax,[ecx+PARENT] ; pop edi ; pop esi ; ret ; ... ;
/// cmp byte [eax+4],0 (the value kind)
const PARENT_TYPE: &str = "57 85 D2 74 16 8B 79 ?? 8D 34 97 8B 46 FC 8D 76 FC 4A 83 38 45 74 0A 85 D2 75 F0 8B 41 ?? 5F 5E \
                           C3 8B 04 97 85 C0 74 38 83 38 00 75 17 80 78 04 00";
const PARENT_TYPE_START: usize = 5;
const CHILDREN: usize = 7;
const PARENT: usize = 29;
/// Building nodes (the `(global vars)` entry): push 30h (size) ; mov dword [ebx],<vars> ; call alloc ; add esp,4 ; ... ;
/// mov ecx,eax ; call ctor (thiscall) ; ... ; push eax ; mov ecx,ebx ; ... ; call add_child (thiscall: parent, child)
const NODES: &str = "6A ?? C7 03 ?? ?? 00 00 E8 ?? ?? ?? ?? 83 C4 04 89 45 20 C6 45 FC 04 85 C0 74 09 8B C8 E8 ?? ?? ?? \
                     ?? EB 02 33 C0 50 8B CB 89 45 20 C6 45 FC 00 E8";
const NODES_SIZE: usize = 1;
const NODES_ID: usize = 4;
const NODES_ALLOC: usize = 8;
const NODES_CTOR: usize = 29;
const NODES_ADD: usize = 48;
/// The allocator: mov ecx,pool ; jmp alloc
const ALLOC: &str = "B9 ?? ?? ?? ?? E9";
/// The constructor: push ebp ; mov ebp,esp ; push -1 ; push handler ; ... ; mov esi,ecx ; mov [ebp-10h],esi ;
/// push 0 ; push 0 ; lea ecx,[esi+0Ch]
const CTOR: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 51 56 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 \
                    00 00 8B F1 89 75 F0 6A 00 6A 00 8D 4E 0C";
/// The child append: push ebp ; mov ebp,esp ; push esi ; mov esi,ecx ; movzx ecx,word [esi+COUNT] ;
/// movzx eax,word [esi+CAPACITY] ; cmp cx,ax ; jb ; ... ; mov [eax+PARENT],esi
const ADD_CHILD: &str = "55 8B EC 56 8B F1 0F B7 4E ?? 0F B7 46 ?? 66 3B C8 72 ?? 83 C0 05";
const ADD_COUNT: usize = 9;
const ADD_CAPACITY: usize = 13;

/// The stock parent-type pass.
static PASS_ORIG: Foreign<unsafe extern "C" fn(usize)> = Foreign::new();

/// The parent-type pass, after [`unwrap_finals`].
///
/// # Safety
/// Only called by the compiler, with its tree root.
unsafe extern "C" fn parent_pass(root: usize) {
    // SAFETY: the compiler's root, before the definition passes (caller).
    unsafe {
        unwrap_finals(root);
        PASS_ORIG.get()(root);
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let names = Names::new(core)?;
    let ids = Ids::new(&names)?;
    let pass = core.find_unique(PASS, "parent-type pass call")?;
    ensure!(core.c_str(core.ptr_at(pass + PASS_LOG)?)? == b"Assigning parent types\n", "parent-type pass: not its log");
    let pass_fn = core.follow_call(pass + PASS_CALL, PASS_FN, "parent-type pass")?;
    let site = core.find_unique(SITE, "override check")?;
    ensure!(
        core.u32_at(site + SITE_PROC)? == ids.proc && core.u32_at(site + SITE_VERB)? == ids.verb,
        "override check: not the proc and verb ids"
    );
    let lookup_fn = core.follow_call(site + SITE_CALL, LOOKUP, "override check: lookup")?;
    let defined = core.find_unique_in(site, 0x100, DEFINED, "override check: defined test")?;
    let p = core.find_unique(PARENT_TYPE, "parent type")?;
    let nodes = core.find_unique(NODES, "node building")?;
    ensure!(names.name(core.u32_at(nodes + NODES_ID)?)? == Some(&b"vars"[..]), "node building: not the vars node");
    let alloc = core.follow_call(nodes + NODES_ALLOC, ALLOC, "node allocator")?;
    let ctor = core.follow_call(nodes + NODES_CTOR, CTOR, "node constructor")?;
    let add_child = core.follow_call(nodes + NODES_ADD, ADD_CHILD, "child append")?;
    let layout = Layout {
        parent: usize::from(core.u8_at(p + PARENT)?),
        children: usize::from(core.u8_at(p + CHILDREN)?),
        count: usize::from(core.u8_at(add_child + ADD_COUNT)?),
        flags: usize::from(core.u8_at(defined + DEFINED_FLAGS)?),
        ids,
    };
    // The child array, its capacity, and its count, in that order.
    ensure!(
        layout.count == layout.children + 6
            && usize::from(core.u8_at(add_child + ADD_CAPACITY)?) == layout.children + 4,
        "child append: another child array layout"
    );
    PASS_ORIG.set(pass_fn);
    FOUND.commit(
        cx,
        Found {
            layout,
            size: usize::from(core.u8_at(nodes + NODES_SIZE)?),
            alloc: Foreign::at(alloc),
            ctor: Foreign::at(ctor),
            add_child: Foreign::at(add_child),
            parent_type: Foreign::at(p - PARENT_TYPE_START),
            lookup: Foreign::at(lookup_fn),
        },
    );
    core.retarget(pass + PASS_CALL + 1, parent_pass as unsafe extern "C" fn(usize))?;
    core.retarget(site + SITE_CALL + 1, lookup as Lookup)?;
    Ok(core.report(&[pass + PASS_CALL, site + SITE_CALL]))
}
