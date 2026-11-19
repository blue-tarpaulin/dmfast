//! `overridetype`, Linux: path resolution's call to itself for a matched child goes through [`super::step`], the
//! duplicate check's count through [`super::duplicates`].

use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// Path resolution's step into a matched child (a created one has its own call): push [ebp-34h] (flags) ;
/// push [ebp-2Ch] ; push [ebp+0Ch] (path) ; push edx (child) ; call resolve ; add word [ebp-30h],1 (the next child
/// with the name) ; add esp,10h ; test eax,eax ; je retry
const STEP: &str = "FF 75 CC FF 75 D4 FF 75 0C 52 E8 ?? ?? ?? ?? 66 83 45 D0 01 83 C4 10 85 C0 0F 84";
const STEP_CALL: usize = 10;
/// Path resolution (cdecl): push ebp/edi/esi/ebx ; sub esp,3Ch ; movzx eax,byte [ebp+14h] (flags) ;
/// mov ebx,[ebp+8] (node) ; mov esi,[ebp+10h] ; mov [ebp-2Dh],al ; test ebx,ebx ; je ; mov ecx,[ebp+0Ch] ;
/// test ecx,ecx ; je
const RESOLVE: &str =
    "55 89 E5 57 56 53 83 EC 3C 0F B6 45 14 8B 5D 08 8B 75 10 88 45 D3 85 DB 0F 84 ?? ?? ?? ?? 8B 4D 0C 85 C9 0F 84";
/// The definition pass's duplicate check: sub esp,0Ch ; push <node> ; call count ; add esp,10h ; cmp ax,1 ; ja error,
/// where error is sub esp,8 ; push "duplicate definition" ; push <node> ; call
const CHECK: &str = "83 EC 0C ?? E8 ?? ?? ?? ?? 83 C4 10 66 83 F8 01 77";
const CHECK_NODE: usize = 3;
const CHECK_CALL: usize = 4;
const CHECK_JA: usize = 16;
const ERROR: &str = "83 EC 08 68 ?? ?? ?? ?? ?? E8";
const ERROR_MESSAGE: usize = 4;
const ERROR_NODE: usize = 8;
/// The count of a node's siblings with its name (cdecl): mov esi,[esp+0Ch] ; mov eax,[esi+PARENT] ; ... ;
/// movzx edx,word [eax+COUNT] ; ... ; mov ebx,[eax+CHILDREN]
const COUNT: &str = "56 53 8B 74 24 0C 8B 46 ?? 85 C0 74 ?? 0F B7 50 ?? 8D 4A FF 66 85 D2 74 ?? 8B 58 ??";
/// Where it reads the parent, child array, and child count offsets.
const COUNT_LAYOUT: [usize; 3] = [8, 27, 16];

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let step = core.find_unique(STEP, "path resolution step")? + STEP_CALL;
    let resolve = core.follow_call(step, RESOLVE, "path resolution")?;
    ensure!((resolve..resolve + 0x400).contains(&step), "path resolution step: not its call to itself");
    let check = core.find_unique(CHECK, "duplicate check")?;
    let error = core.branch_at(check + CHECK_JA)?.target;
    core.require(error, ERROR, "duplicate check: error")?;
    ensure!(
        core.c_str(core.ptr_at(error + ERROR_MESSAGE)?)? == b"duplicate definition"
            && core.u8_at(error + ERROR_NODE)? == core.u8_at(check + CHECK_NODE)?,
        "duplicate check: not its message"
    );
    let count = core.follow_call(check + CHECK_CALL, COUNT, "duplicate count")?;
    let mut layout = [0; 3];
    for (l, o) in layout.iter_mut().zip(COUNT_LAYOUT) {
        *l = core.u8_at(count + o)?;
    }
    super::commit(cx, layout, resolve, step + 1, check + CHECK_CALL + 1)?;
    Ok(core.report(&[step, check + CHECK_CALL]))
}
