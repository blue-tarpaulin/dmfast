//! `procargs`, Linux: the argument compile pass (cdecl: compiler, node) and the tree insertion (regparm(3): eax =
//! path, edx = parent, ecx = flag; [`insert`] adapts it) are hooked at their entries.

use std::arch::naked_asm;

use super::{FOUND, Found, check_path, is_var, report_duplicates};
use crate::base::code::{Addr, Foreign};
use crate::base::feature::Ctx;
use crate::base::image::Hits;
use crate::base::{Result, ensure};
use crate::compiler::layout::CHILDREN;

type Compile = unsafe extern "C" fn(usize, usize);

static COMPILE: Foreign<Compile> = Foreign::new();
/// The stock tree insertion, for [`insert`].
static INSERT: Addr = Addr::new();

/// The argument compile pass: push ebp,edi,esi,ebx ; sub esp,3Ch ; mov ebx,[esp+54h] ; mov esi,[esp+50h] ;
/// cmp byte [ebx+TAG],VAR ; movzx edx,word [ebx+COUNT] ; je var
const COMPILE_FN: &str = "55 57 56 53 83 EC 3C 8B 5C 24 54 8B 74 24 50 80 7B 10 11 0F B7 53 1E 74";
const COMPILE_VAR: usize = 24;
/// push ebp,edi,esi,ebx ; sub esp,3Ch ; mov ebx,[esp+54h]: 11 position-independent bytes.
const COMPILE_ENTRY: usize = 11;
/// For a var node: mov eax,[ebx+NAME] (the argument's name).
const VAR_NAME: &str = "8B 43 0C";
/// For any other node, one call per child: mov eax,[ebx+CHILDREN] ; sub esp,8 ; push [eax+edi] ; add edi,4 ;
/// push esi ; call itself
const KIDS: &str = "8B 43 ?? 83 EC 08 FF 34 38 83 C7 04 56 E8 ?? ?? ?? ??";
const KIDS_CHILDREN: usize = 2;
const KIDS_CALL: usize = 13;
/// push <"undefined argument type"> ; push ebx ; call error ; add esp,10h
const ERROR_CALL: &str = "68 ?? ?? ?? ?? 53 E8 ?? ?? ?? ?? 83 C4 10";
const ERROR_AT: usize = 6;
/// The error function is printf-style cdecl: push ebp ; mov ebp,esp ; push edi ; push esi ; push ebx ;
/// sub esp,1Ch ; mov ebx,[ebp+8] (node) ; test ebx,ebx. Callers pop their own arguments (`add esp,10h`).
const ERROR_FN: &str = "55 89 E5 57 56 53 83 EC 1C 8B 5D 08 85 DB";
/// The tree insertion: push ebp ; mov ebp,esp ; push edi ; mov edi,edx ; push esi ; push ebx ; mov ebx,eax ;
/// sub esp,imm8 ; cmp byte [flag],0 ; mov [ebp-x],cl ; je ... ; cmp byte [flag],0 ; jne ...
const INSERT_FN: &str =
    "55 89 E5 57 89 D7 56 53 89 C3 83 EC ?? 80 3D ?? ?? ?? ?? 00 88 4D ?? 74 ?? 80 3D ?? ?? ?? ?? 00 0F 85";
/// push ebp ; mov ebp,esp ; push edi ; mov edi,edx: 6 position-independent bytes.
const INSERT_ENTRY: usize = 6;
/// The ids, from the insertion's code: the `var` scan (cmp [base+index*4],imm32 ; je), checked against the parent
/// walk (cmp edx,imm32 ; je ; mov eax,[eax+PARENT] ; test eax,eax ; je ; mov edx,[eax]), and the argument list test
/// (mov edx,[ebx] ; mov eax,ecx ; or eax,10h ; cmp [edx],imm32 ; cmove ecx,eax).
const VAR_SCAN: &str = "81 3C ?? ?? ?? ?? ?? 0F 84";
const VAR_SCAN_KIND: usize = 3;
const VAR_WALK: &str = "81 FA ?? ?? ?? ?? 0F 84 ?? ?? ?? ?? 8B 40 14 85 C0 74 ?? 8B 10";
const VAR_WALK_KIND: usize = 2;
const LIST_TEST: &str = "8B 13 89 C8 83 C8 10 81 3A ?? ?? ?? ?? 0F 44 C8";
const LIST_TEST_ID: usize = 9;
/// `/`, from the helper the insertion calls with (parent, path, flag): mov eax,[esp+14h] ; test eax,eax ; je ;
/// mov ecx,[eax+8] ; ... ; jne ; mov esi,[eax] ; xor ecx,ecx ; cmp dword [esi],imm8 ; jne
const SLASH_TEST: &str = "56 53 83 EC 04 8B 44 24 14 8B 5C 24 18 85 C0 74 ?? 8B 48 08 89 DA 85 C9 75 ?? 0F BE D2 52 \
                          51 50 FF 74 24 1C E8 ?? ?? ?? ?? 83 C4 10 83 C4 04 5B 5E C3 8D B6 00 00 00 00 8B 30 31 C9 \
                          83 3E ?? 75";
const SLASH_TEST_KIND: usize = 62;

/// The argument compile pass: a non-var node's duplicate names are reported first.
///
/// # Safety
/// BYOND's call: the proc compiler and a live node.
unsafe extern "C" fn compile(this: usize, node: usize) {
    // SAFETY: a live node (caller); the original was set at install.
    unsafe {
        if !is_var(node) {
            report_duplicates(node);
        }
        COMPILE.get()(this, node)
    }
}

/// [`check_path`] for the tree insertion's entry (eax = path, edx = parent, ecx = flag), then the stock insertion.
///
/// # Safety
/// Only jumped to from the tree insertion's entry.
#[unsafe(naked)]
unsafe extern "C" fn insert() {
    naked_asm!(
        "pushad",
        "mov esi, esp",
        "and esp, -16",
        "sub esp, 8",
        "push edx",
        "push eax",
        "call {check_path}",
        "mov esp, esi",
        "popad",
        "jmp dword ptr [{insert}]",
        check_path = sym check_path,
        insert = sym INSERT,
    )
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(COMPILE_FN, "proc argument compile")?;
    core.require(core.rel8(f + COMPILE_VAR)?, VAR_NAME, "proc argument compile: var name read")?;
    let kids: Hits = core
        .find_in(f, 0x100, KIDS)?
        .iter()
        .copied()
        .filter(|&k| core.call_target(k + KIDS_CALL).is_ok_and(|t| t == f))
        .collect();
    let kids = kids.unique("argument children loop")?;
    ensure!(
        usize::from(core.u8_at(kids + KIDS_CHILDREN)?) == CHILDREN,
        "argument children loop: another child array offset"
    );
    let calls = core.find_in(f, 0x500, ERROR_CALL)?;
    let error = core.call_target(calls.first_or("argument error call")? + ERROR_AT)?;
    for &c in &calls {
        ensure!(core.call_target(c + ERROR_AT)? == error, "argument error calls differ");
    }
    core.require(error, ERROR_FN, "argument error function")?;
    let ins = core.find_unique(INSERT_FN, "tree insertion")?;
    let var = core.u32_at(core.find_unique_in(ins, 0x400, VAR_SCAN, "var scan")? + VAR_SCAN_KIND)?;
    let walk = core.find_unique_in(ins, 0x400, VAR_WALK, "var parent walk")?;
    ensure!(core.u32_at(walk + VAR_WALK_KIND)? == var, "tree insertion: var kinds differ");
    let arg_list = core.u32_at(core.find_unique_in(ins, 0x400, LIST_TEST, "arglist test")? + LIST_TEST_ID)?;
    let helper = core.find_unique(SLASH_TEST, "absolute path test")?;
    ensure!(!core.calls_in(ins, 0x400, helper)?.is_empty(), "absolute path test: not called by the tree insertion");
    // An imm8, sign-extended by the cmp.
    let slash = core.u8_at(helper + SLASH_TEST_KIND)? as i8 as u32;
    INSERT.set(core.hook(ins, INSERT_ENTRY, insert as unsafe extern "C" fn())?);
    COMPILE.set(core.hook(f, COMPILE_ENTRY, compile as Compile)?);
    FOUND.commit(cx, Found { error: Foreign::at(error), arg_list, var, slash });
    Ok(core.report(&[f]))
}
