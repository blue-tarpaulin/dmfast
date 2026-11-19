//! `mutableref`, Windows: the appearance assignment's calls go to [`assign`], the copy's take and release of its own
//! references are hooked, and the render var writes' conversions go to [`source_stub`] and [`target_stub`].

use super::{FOUND, Found, TO_STRING, assigned, owns, releasing, render_stub, took};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

type Assign = unsafe extern "thiscall" fn(usize, u32);
type Refs = unsafe extern "fastcall" fn(usize);

static ASSIGN: Foreign<Assign> = Foreign::new();
static TAKE: Foreign<Refs> = Foreign::new();
static RELEASE: Foreign<Refs> = Foreign::new();

/// The appearance assignment (thiscall: the copy, the appearance id): push ebp ; mov ebp,esp ; push ecx ;
/// mov eax,[appearances] ; (look up the appearance) ; mov ecx,ebx ; call release ; mov ecx,ebx ; call drop_lists ;
/// cmp byte [ebx+10Dh],0 ; je ; (hold the new one) push eax ; mov [ebx+104h],eax ; call hold. Its calls go to
/// [`assign`]: the entry loads an address the loader relocates, so it can't be hooked.
const ASSIGN_IDA: &str = "55 8B EC 51 A1 ?? ?? ?? ?? 53 8B D9 8B 4D 08 56 57 3B 48 44 73 08 8B 40 40 8B 3C 88 EB 02 \
                          33 FF 85 FF 89 7D 08 BE FF FF 00 00 0F 45 F1 8B CB 89 75 FC E8 ?? ?? ?? ?? 8B CB E8 ?? ?? \
                          ?? ?? 80 BB 0D 01 00 00 00 74 ?? 8B 45 FC 8B B3 04 01 00 00 50 89 83 04 01 00 00 E8";
const ASSIGN_RELEASE: usize = 0x32;
const ASSIGN_HOLD: usize = 0x57;
/// The release (fastcall, the copy): push esi ; mov esi,ecx ; cmp byte [esi+10Ch],0 ; je done ; call drop_lists ;
/// push [esi+1Ch] ; mov byte [esi+10Ch],0 ; call string_release
const RELEASE_IDA: &str =
    "56 8B F1 80 BE 0C 01 00 00 00 0F 84 ?? ?? ?? ?? E8 ?? ?? ?? ?? FF 76 1C C6 86 0C 01 00 00 00 E8";
const RELEASE_STRING: usize = 0x1F;
/// The take (a no-op when the copy has its own references): push esi ; mov esi,ecx ; cmp byte [esi+10Ch],0 ;
/// jne done ; push edi ; mov edi,[esi+1Ch] ; push edi ; mov byte [esi+10Ch],1 ; call string_entry
const TAKE_IDA: &str = "56 8B F1 80 BE 0C 01 00 00 00 0F 85 ?? ?? ?? ?? 57 8B 7E 1C 57 C6 86 0C 01 00 00 01 E8";
/// Both start with push esi ; mov esi,ecx ; cmp byte [esi+10Ch],0: 10 position-independent bytes.
const ENTRY: usize = 10;
/// In the appearance hold, its first use adds a reference on the appearance's strings: test ecx,ecx ; jne done ;
/// push [esi+1Ch] ; call string_add ; push [esi+20h] ; call string_add
const HOLD_ADD: &str = "85 C9 75 ?? FF 76 1C E8 ?? ?? ?? ?? FF 76 20 E8";
const HOLD_ADD_CALL: usize = 7;
/// The string add: push ebp ; mov ebp,esp ; push esi ; mov esi,[ebp+8] ; push esi ; call string_entry ;
/// add esp,4 ; test eax,eax ; je ; inc dword [eax+10h]
const ADD_IDA: &str = "55 8B EC 56 8B 75 08 56 E8 ?? ?? ?? ?? 83 C4 04 85 C0 74 ?? FF 40 10";
/// The copy's var write of render_source and render_target (edi = the copy): mov ecx,[ebp-2Ch] ; push 6 ;
/// push esi ; push ecx ; call to_string ; add esp,0Ch ; mov [edi+field],eax ; jmp done
const SOURCE_WRITE: &str = "8B 4D D4 6A 06 56 51 E8 ?? ?? ?? ?? 83 C4 0C 89 87 BC 00 00 00 E9";
const TARGET_WRITE: &str = "8B 4D D4 6A 06 56 51 E8 ?? ?? ?? ?? 83 C4 0C 89 87 C0 00 00 00 E9";
const WRITE_CALL: usize = 7;
/// A new mutable appearance's first publish (fastcall, the copy): push esi ; mov esi,ecx ; mov edx,[esi+0ECh] ;
/// lea eax,[edx+1] ; mov [esi+0ECh],eax ; test edx,edx ; jne done ; push [esi+108h] ; call hold
const PUBLISH: &str = "56 8B F1 8B 96 EC 00 00 00 8D 42 01 89 86 EC 00 00 00 85 D2 75 ?? FF B6 08 01 00 00 E8";
const PUBLISH_HOLD: usize = 0x1C;

/// The appearance assignment, then [`assigned`].
unsafe extern "thiscall" fn assign(copy: usize, id: u32) {
    // SAFETY: BYOND's call, passed on, on its working copy.
    unsafe {
        ASSIGN.get()(copy, id);
        assigned(copy);
    }
}

/// The take, then [`took`] if it gave the copy its own references.
unsafe extern "fastcall" fn take(copy: usize) {
    // SAFETY: BYOND's call, passed on, on its working copy.
    unsafe {
        let owned = owns(copy);
        TAKE.get()(copy);
        if !owned {
            took(copy);
        }
    }
}

/// [`releasing`], then the release.
unsafe extern "fastcall" fn release(copy: usize) {
    releasing(copy);
    // SAFETY: BYOND's call, passed on, on its working copy.
    unsafe { RELEASE.get()(copy) }
}

render_stub!(source_stub, super::set_source, "edi");
render_stub!(target_stub, super::set_target, "edi");

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let assign_fn = core.find_unique(ASSIGN_IDA, "appearance assignment")?;
    let release_fn = core.follow_call(assign_fn + ASSIGN_RELEASE, RELEASE_IDA, "appearance assignment: release")?;
    let hold = core.call_target(assign_fn + ASSIGN_HOLD)?;
    let add_at = core.find_unique_in(hold, 0x100, HOLD_ADD, "appearance hold: string add")?;
    let take_fn = core.find_unique(TAKE_IDA, "working appearance: take references")?;
    let source = core.find_unique(SOURCE_WRITE, "working appearance: render_source write")?;
    let target = core.find_unique(TARGET_WRITE, "working appearance: render_target write")?;
    let publish = core.find_unique(PUBLISH, "mutable appearance publish")?;
    ensure!(core.call_target(publish + PUBLISH_HOLD)? == hold, "mutable appearance publish: another hold");
    let to_string = core.call_target(source + WRITE_CALL)?;
    ensure!(core.call_target(target + WRITE_CALL)? == to_string, "render var writes: different conversions");
    FOUND.commit(
        cx,
        Found {
            hold: Foreign::at(hold),
            add: Foreign::at(core.follow_call(add_at + HOLD_ADD_CALL, ADD_IDA, "string add")?),
            release: Foreign::at(core.call_target(release_fn + RELEASE_STRING)?),
        },
    );
    TO_STRING.set(to_string);
    ASSIGN.set(assign_fn);
    core.redirect_calls(assign_fn, assign as Assign)?;
    TAKE.set(core.hook(take_fn, ENTRY, take as Refs)?);
    RELEASE.set(core.hook(release_fn, ENTRY, release as Refs)?);
    core.retarget(source + WRITE_CALL + 1, source_stub as unsafe extern "C" fn())?;
    core.retarget(target + WRITE_CALL + 1, target_stub as unsafe extern "C" fn())?;
    core.write_code(publish + PUBLISH_HOLD, &super::NO_CALL)?;
    Ok(core.report(&[assign_fn, take_fn, release_fn]))
}
