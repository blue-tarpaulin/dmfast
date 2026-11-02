//! `procargs`, Windows: the argument compile pass (thiscall: compiler, node) and the tree insertion (cdecl: path,
//! parent, flag) are hooked at their entries.

use super::{FOUND, Found, check_path, is_var, report_duplicates};
use crate::base::code::{Foreign, read};
use crate::base::feature::Ctx;
use crate::base::image::Hits;
use crate::base::{Result, ensure};
use crate::compiler::layout::{CHILDREN, ID};

type Compile = unsafe extern "thiscall" fn(usize, usize);
type Insert = unsafe extern "C" fn(usize, usize, usize) -> usize;

static COMPILE: Foreign<Compile> = Foreign::new();
static INSERT: Foreign<Insert> = Foreign::new();

/// The argument compile pass: push ebp ; mov ebp,esp ; mov eax,[ebp+8] ; sub esp,1Ch ; push ebx ;
/// movzx ebx,word [eax+COUNT] ; push esi ; xor esi,esi ; cmp byte [eax+TAG],VAR ; push edi ; mov edi,ecx ; jne ... ;
/// mov ecx,[eax+NAME] ; push eax ; mov [ebp-4],esi
const COMPILE_FN: &str =
    "55 8B EC 8B 45 08 83 EC 1C 53 0F B7 58 22 56 33 F6 80 78 10 11 57 8B F9 0F 85 ?? ?? ?? ?? 8B 48 0C 50 89 75 FC";
/// push ebp ; mov ebp,esp ; mov eax,[ebp+8]: 6 position-independent bytes.
const COMPILE_ENTRY: usize = 6;
/// In it: push <"undefined argument type"> ; push ebx ; call error ; add esp,8 ; cmp [ebp-0Ch],0
const ERROR_CALL: &str = "68 ?? ?? ?? ?? 53 E8 ?? ?? ?? ?? 83 C4 08 83 7D F4 00";
const ERROR_AT: usize = 6;
/// And the loop over a non-var node's children: mov eax,[ebp+8] ; movzx ecx,si ; mov eax,[eax+CHILDREN] ;
/// push [eax+ecx*4] ; mov ecx,edi ; call itself
const KIDS: &str = "8B 45 08 0F B7 CE 8B 40 ?? FF 34 88 8B CF E8 ?? ?? ?? ??";
const KIDS_CHILDREN: usize = 8;
const KIDS_CALL: usize = 14;
/// The tree insertion: SEH prologue ; cmp [..],0 ; mov ebx,[ebp+8] (the path) ; je ; cmp [..],0 ; je ; cmp [..],0 ;
/// jne ; mov eax,[ebp+0Ch] ; <epilogue> ; ret ; mov eax,[ebx+40h] ; mov edx,[ebx+8] (its length)
const INSERT_FN: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 0C 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 \
                         8D 45 F4 64 A3 00 00 00 00 80 3D ?? ?? ?? ?? 00 8B 5D 08 74 27 80 3D ?? ?? ?? ?? 00 74 1E \
                         80 3D ?? ?? ?? ?? 00 75 15 8B 45 0C 8B 4D F4 64 89 0D 00 00 00 00 59 5F 5E 5B 8B E5 5D C3 \
                         8B 43 40 8B 53 08";
/// push ebp ; mov ebp,esp ; push -1: 5 position-independent bytes.
const INSERT_ENTRY: usize = 5;
/// The ids, from the insertion's code: the `var` scan (cmp [eax+ecx*4],imm32 ; je), checked against the parent walk
/// (mov ecx,[node] ; test ecx,ecx ; je ; cmp ecx,imm32 ; je ; mov eax,[eax+PARENT]); the argument list test
/// (mov eax,[ebx] ; ... ; cmp [eax],imm32 ; jne); the absolute path test (test edx,edx ; je ; mov eax,[ebx] ;
/// cmp dword [eax],imm8 ; ... ; jne).
const VAR_SCAN: &str = "81 3C ?? ?? ?? ?? ?? 74";
const VAR_SCAN_KIND: usize = 3;
const VAR_WALK: &str = "8B 08 85 C9 74 ?? 81 F9 ?? ?? ?? ?? 74 ?? 8B 40 18";
const VAR_WALK_KIND: usize = 8;
const LIST_TEST: &str = "8B 03 8B 4D ?? 88 4D 10 81 38 ?? ?? ?? ?? 75";
const LIST_TEST_ID: usize = 10;
const SLASH_TEST: &str = "85 D2 74 ?? 8B 03 83 38 ?? 8A 45 ?? 75";
const SLASH_TEST_KIND: usize = 8;

/// The argument compile pass: an argument list's duplicate names are reported first.
///
/// # Safety
/// BYOND's call: the proc compiler and a live node.
unsafe extern "thiscall" fn compile(this: usize, node: usize) {
    // SAFETY: a live node (caller); the original and the ids were set at install.
    unsafe {
        if read::<u32>(node + ID) == FOUND.get().arg_list && !is_var(node) {
            report_duplicates(node);
        }
        COMPILE.get()(this, node)
    }
}

/// The tree insertion: a `/var/` path below an argument list is reported first.
///
/// # Safety
/// BYOND's call: a live path vector, a live parent node or null, and a flag.
unsafe extern "C" fn insert(path: usize, parent: usize, flag: usize) -> usize {
    // SAFETY: as the caller's; the original was set at install.
    unsafe {
        check_path(path, parent);
        INSERT.get()(path, parent, flag)
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let f = core.find_unique(COMPILE_FN, "proc argument compile")?;
    let error = core.call_target(core.find_unique_in(f, 0x300, ERROR_CALL, "argument error call")? + ERROR_AT)?;
    let kids: Hits = core
        .find_in(f, 0x400, KIDS)?
        .iter()
        .copied()
        .filter(|&k| core.call_target(k + KIDS_CALL).is_ok_and(|t| t == f))
        .collect();
    let kids = kids.unique("argument children loop")?;
    ensure!(
        usize::from(core.u8_at(kids + KIDS_CHILDREN)?) == CHILDREN,
        "argument children loop: another child array offset"
    );
    let ins = core.find_unique(INSERT_FN, "tree insertion")?;
    let var = core.u32_at(core.find_unique_in(ins, 0x400, VAR_SCAN, "var scan")? + VAR_SCAN_KIND)?;
    let walk = core.find_unique_in(ins, 0x400, VAR_WALK, "var parent walk")?;
    ensure!(core.u32_at(walk + VAR_WALK_KIND)? == var, "tree insertion: var kinds differ");
    let arg_list = core.u32_at(core.find_unique_in(ins, 0x400, LIST_TEST, "arglist test")? + LIST_TEST_ID)?;
    let slash = core.find_unique_in(ins, 0x400, SLASH_TEST, "slash test")?;
    // An imm8, sign-extended by the cmp.
    let slash = core.u8_at(slash + SLASH_TEST_KIND)? as i8 as u32;
    INSERT.set(core.hook(ins, INSERT_ENTRY, insert as Insert)?);
    COMPILE.set(core.hook(f, COMPILE_ENTRY, compile as Compile)?);
    FOUND.commit(cx, Found { error: Foreign::at(error), arg_list, var, slash });
    Ok(core.report(&[f]))
}
