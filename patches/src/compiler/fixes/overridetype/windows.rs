//! `overridetype`, Windows: path resolution's call to itself goes through [`super::step`], the duplicate check's
//! count through [`super::duplicates`].

use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// Path resolution's step into a matched or created child: mov al,[ebp+14h] (flags) ; mov edi,[ebp+10h] ;
/// and al,0F8h ; movzx eax,al ; push eax ; push edi ; push [ebp+0Ch] (path) ; push esi (child) ; call resolve ;
/// add esp,10h ; test eax,eax ; jne done ; cmp [ebp-0Dh],al (created) ; je fail
const STEP: &str = "8A 45 14 8B 7D 10 24 F8 0F B6 C0 50 57 FF 75 0C 56 E8 ?? ?? ?? ?? 83 C4 10 85 C0 75 05 38 45 F3 74";
const STEP_CALL: usize = 17;
/// Path resolution (cdecl): its SEH prologue, then test esi,esi (node) ; je ; mov edx,[ebp+0Ch] ; test edx,edx
const RESOLVE: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 18 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 \
                       F4 64 A3 00 00 00 00 8B 75 08 85 F6 0F 84 ?? ?? ?? ?? 8B 55 0C 85 D2";
/// The definition pass's duplicate check of a type not yet defined: test byte [esi+14h],1 ; jne ; push esi ;
/// call count ; add esp,4 ; cmp ax,1 ; jbe ; push "duplicate definition" ; push esi ; call error
const CHECK: &str = "F6 46 14 01 75 ?? 56 E8 ?? ?? ?? ?? 83 C4 04 66 83 F8 01 76 ?? 68 ?? ?? ?? ?? 56 E8";
const CHECK_CALL: usize = 7;
const CHECK_MESSAGE: usize = 22;
/// The count of a node's siblings with its name (cdecl): mov ecx,[ebp+8] ; ... ; mov eax,[ecx+PARENT] ; ... ;
/// movzx esi,word [eax+COUNT] ; ... ; mov ebx,[eax+CHILDREN]
const COUNT: &str =
    "55 8B EC 8B 4D 08 57 33 FF 8B 41 ?? 85 C0 75 03 5F 5D C3 56 0F B7 70 ?? 66 85 F6 74 ?? 53 8B 58 ??";
/// Where it reads the parent, child array, and child count offsets.
const COUNT_LAYOUT: [usize; 3] = [11, 32, 23];

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let step = core.find_unique(STEP, "path resolution step")? + STEP_CALL;
    let resolve = core.follow_call(step, RESOLVE, "path resolution")?;
    ensure!((resolve..resolve + 0x400).contains(&step), "path resolution step: not its call to itself");
    let check = core.find_unique(CHECK, "duplicate check")?;
    ensure!(
        core.c_str(core.ptr_at(check + CHECK_MESSAGE)?)? == b"duplicate definition",
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
