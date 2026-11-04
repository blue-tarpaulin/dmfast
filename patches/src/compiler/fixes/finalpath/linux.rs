//! `finalpath`, Linux: the parent-type pass's call goes through [`parent_pass`], the override check's lookup call
//! through [`lookup`].

use std::arch::naked_asm;

use super::{FOUND, Found, Ids, Layout, Lookup, lookup, unwrap_finals};
use crate::base::code::{Addr, Foreign};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};
use crate::compiler::names::Names;

/// Before the definition passes: push "Assigning parent types\n" ; call log ; add esp,10h ; mov eax,[root] ;
/// call pass ; mov eax,[root] ; movzx edx,word [eax+1Eh]
const PASS: &str = "68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 A1 ?? ?? ?? ?? E8 ?? ?? ?? ?? A1 ?? ?? ?? ?? 0F B7 50 1E";
const PASS_LOG: usize = 1;
const PASS_CALL: usize = 18;
/// The parent-type pass (node in eax): push ebp/edi/esi/ebx ; sub esp,3Ch ; mov [esp+28h],eax ; test eax,eax ; je
const PASS_FN: &str = "55 57 56 53 83 EC 3C 89 44 24 28 85 C0 0F 84";
/// The override check (inlined in the proc definition): lookup(&node, &flags, &builtin) ; if built in: ... ;
/// if found: ...
const SITE: &str = "8D 85 FB FE FF FF 8D BD 00 FF FF FF 89 9D FC FE FF FF 50 8D 85 FC FE FF FF 57 50 C6 85 FB FE FF FF 00 \
                    E8 ?? ?? ?? ?? 83 C4 10 80 BD FB FE FF FF 00 0F 85 ?? ?? ?? ?? 84 C0 0F 85";
const SITE_CALL: usize = 34;
/// For an override (no `proc` or `verb` above it), the flags' final bit (200000h) gives the errors:
/// test byte [ebp-0FEh],20h ; je ; test eax,eax ; je ; sub esp,8 ; push "override not allowed"
const FINAL_TEST: &str = "F6 85 02 FF FF FF 20 0F 84 ?? ?? ?? ?? 85 C0 74 ?? 83 EC 08 68";
const FINAL_TEST_MESSAGE: usize = 21;
/// Then, the definition found must be defined (flags bit 0) or the check is tried again in the next pass:
/// test eax,eax ; je ; test byte [eax+FLAGS],1 ; je ; sub esp,0Ch ; push [eax+0Ch] ; call
const DEFINED: &str = "85 C0 0F 84 ?? ?? ?? ?? F6 40 ?? 01 74 ?? 83 EC 0C FF 70 0C E8";
const DEFINED_FLAGS: usize = 10;
/// The lookup (cdecl): push edi/esi/ebx ; mov esi,[esp+10h] ; mov ebx,[esi] ; test ebx,ebx ; je
const LOOKUP: &str = "57 56 53 8B 74 24 10 8B 1E 85 DB 0F 84";
/// A node's parent type (cdecl): its last `..` child's type, else its parent node. Matched after its first 8 bytes
/// (mov ecx,[esp+4] ; movzx eax,word [ecx+count]), which the parent cache may hook: ... ; mov edx,[ecx+CHILDREN] ;
/// ... ; cmp dword [edx],45h ; ... ; mov eax,[ecx+PARENT] ; ret ; ... ; jmp link_type
const PARENT_TYPE: &str = "83 E8 01 EB 11 8D 76 00 8B 51 ?? 8B 14 82 83 E8 01 83 3A 45 74 12 83 F8 FF 75 ED 8B 41 ?? C3 8D \
                           B4 26 00 00 00 00 66 90 89 54 24 04 E9";
const PARENT_TYPE_START: usize = 8;
const CHILDREN: usize = 10;
const PARENT: usize = 29;
const PARENT_TYPE_LINK: usize = 45;
/// The `..` child's type: push ebx ; ... ; follows nameless one-child nodes without a value (value kind byte at 4) to
/// the path it holds.
const LINK_TYPE: &str = "53 31 D2 83 EC 08 8B 44 24 10 85 C0 0F 84 ?? ?? ?? ?? 8B 08 85 C9 74 1A EB 1E 8D B6 00 00 00 00 66 \
                         83 78 ?? 01 75 79 8B 40 ?? 8B 00 8B 10 85 D2 75 06 80 78 04 00";
/// Building nodes (the `(global vars)` entry): mov dword [esi],250h ; mov dword [esp],2Ch (size) ; call alloc ;
/// mov [esp],eax ; mov edi,eax ; call ctor ; pop ecx ; pop eax ; push edi ; push esi ; call add_child (parent, child)
const NODES: &str =
    "C7 06 50 02 00 00 C7 04 24 ?? 00 00 00 E8 ?? ?? ?? ?? 89 04 24 89 C7 E8 ?? ?? ?? ?? 59 58 57 56 E8";
const NODES_SIZE: usize = 9;
const NODES_ALLOC: usize = 13;
const NODES_CTOR: usize = 23;
const NODES_ADD: usize = 32;
/// The allocator (node pool): push ebp ; mov ebp,esp ; push ebx ; sub esp,4 ; mov ebx,[pool]
const ALLOC: &str = "55 89 E5 53 83 EC 04 8B 1D";
/// The constructor: push ebp ; mov ebp,esp ; push esi ; push ebx ; mov ebx,[ebp+8] ; lea eax,[ebx+0Ch] ; sub esp,4 ;
/// push 0 ; push 0 ; push eax ; call
const CTOR: &str = "55 89 E5 56 53 8B 5D 08 8D 43 0C 83 EC 04 6A 00 6A 00 50 E8";
/// The child append: push esi ; push ebx ; sub esp,0Ch ; mov esi,[esp+18h] ; mov ebx,[esp+1Ch] ;
/// lea eax,[esi+CHILDREN] ; push ebx ; push eax ; call push_back ; mov [ebx+PARENT],esi
const ADD_CHILD: &str =
    "56 53 83 EC 0C 8B 74 24 18 8B 5C 24 1C 8D 46 ?? 53 50 E8 ?? ?? ?? ?? 89 73 ?? 83 C4 14 5B 5E C3";
const ADD_CHILDREN: usize = 15;
const ADD_PUSH: usize = 18;
const ADD_PARENT: usize = 25;
/// The child array's append (array, capacity u16, count u16): push esi ; push ebx ; sub esp,4 ; mov ebx,[esp+10h] ;
/// mov esi,[esp+14h] ; movzx edx,word [ebx+COUNT] ; movzx eax,word [ebx+CAPACITY] ; cmp dx,ax ; jb
const PUSH_BACK: &str = "56 53 83 EC 04 8B 5C 24 10 8B 74 24 14 0F B7 53 ?? 0F B7 43 ?? 66 39 C2 72";
const PUSH_COUNT: usize = 16;
const PUSH_CAPACITY: usize = 20;

/// The stock parent-type pass.
static PASS_ORIG: Addr = Addr::new();

/// The parent-type pass (node in eax), after [`unwrap_finals`]. eax, ecx, and edx are kept for it, and the call is
/// 16-byte aligned whatever the caller's alignment.
///
/// # Safety
/// Only called by the compiler, with its tree root in eax.
#[unsafe(naked)]
unsafe extern "C" fn parent_pass() {
    naked_asm!(
        "push eax",
        "push ecx",
        "push edx",
        "push ebx",
        "mov ebx, esp",
        "sub esp, 16",
        "and esp, -16",
        "mov dword ptr [esp], eax",
        "call {unwrap}",
        "mov esp, ebx",
        "pop ebx",
        "pop edx",
        "pop ecx",
        "pop eax",
        "jmp dword ptr [{orig}]",
        unwrap = sym unwrap_finals_c,
        orig = sym PASS_ORIG,
    )
}

/// [`unwrap_finals`] for [`parent_pass`].
///
/// # Safety
/// As [`unwrap_finals`].
unsafe extern "C" fn unwrap_finals_c(root: usize) {
    // SAFETY: as the caller's.
    unsafe { unwrap_finals(root) }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let ids = Ids::new(&Names::new(core)?)?;
    let pass = core.find_unique(PASS, "parent-type pass call")?;
    ensure!(core.c_str(core.ptr_at(pass + PASS_LOG)?)? == b"Assigning parent types\n", "parent-type pass: not its log");
    let pass_fn = core.follow_call(pass + PASS_CALL, PASS_FN, "parent-type pass")?;
    let site = core.find_unique(SITE, "override check")?;
    let lookup_fn = core.follow_call(site + SITE_CALL, LOOKUP, "override check: lookup")?;
    let test = core.find_unique_in(site, 0x600, FINAL_TEST, "override check: final test")?;
    ensure!(
        core.c_str(core.ptr_at(test + FINAL_TEST_MESSAGE)?)? == b"override not allowed",
        "override check: final test without its error"
    );
    let defined = core.find_unique_in(site, 0x600, DEFINED, "override check: defined test")?;
    let p = core.find_unique(PARENT_TYPE, "parent type")?;
    core.require(core.rel32(p + PARENT_TYPE_LINK)?, LINK_TYPE, "parent type link")?;
    let nodes = core.find_unique(NODES, "node building")?;
    let alloc = core.follow_call(nodes + NODES_ALLOC, ALLOC, "node allocator")?;
    let ctor = core.follow_call(nodes + NODES_CTOR, CTOR, "node constructor")?;
    let add_child = core.follow_call(nodes + NODES_ADD, ADD_CHILD, "child append")?;
    let push = core.follow_call(add_child + ADD_PUSH, PUSH_BACK, "child array append")?;
    let children = usize::from(core.u8_at(p + CHILDREN)?);
    let layout = Layout {
        parent: usize::from(core.u8_at(p + PARENT)?),
        children,
        count: children + usize::from(core.u8_at(push + PUSH_COUNT)?),
        flags: usize::from(core.u8_at(defined + DEFINED_FLAGS)?),
        ids,
    };
    ensure!(
        usize::from(core.u8_at(add_child + ADD_CHILDREN)?) == layout.children
            && usize::from(core.u8_at(add_child + ADD_PARENT)?) == layout.parent
            && core.u8_at(push + PUSH_COUNT)? == 6
            && core.u8_at(push + PUSH_CAPACITY)? == 4,
        "child append: another node layout"
    );
    PASS_ORIG.set(pass_fn);
    FOUND.commit(
        cx,
        Found {
            layout,
            size: core.ptr_at(nodes + NODES_SIZE)?,
            alloc: Foreign::at(alloc),
            ctor: Foreign::at(ctor),
            add_child: Foreign::at(add_child),
            parent_type: Foreign::at(p - PARENT_TYPE_START),
            lookup: Foreign::at(lookup_fn),
        },
    );
    core.retarget(pass + PASS_CALL + 1, parent_pass as unsafe extern "C" fn())?;
    core.retarget(site + SITE_CALL + 1, lookup as Lookup)?;
    Ok(core.report(&[pass + PASS_CALL, site + SITE_CALL]))
}
