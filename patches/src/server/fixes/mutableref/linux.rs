//! `mutableref`, Linux: the appearance assignment and the copy's take and release of its own references are hooked,
//! and the render var writes' conversions go to [`source_stub`] and [`target_stub`].

use std::arch::naked_asm;

use super::{FOUND, Found, TO_STRING, assigned, releasing, render_stub, took};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

type Assign = unsafe extern "C" fn(usize, u32);

static ASSIGN: Foreign<Assign> = Foreign::new();
/// The take and release (regparm: eax = the copy), from their moved entries.
static TAKE: Foreign<unsafe extern "C" fn()> = Foreign::new();
static RELEASE: Foreign<unsafe extern "C" fn()> = Foreign::new();

/// The appearance assignment (cdecl: the copy, the appearance id; reached by tail jumps): push ebp ; push edi ;
/// push esi ; push ebx ; sub esp,0Ch ; (look up the appearance) ; cmp byte [ebx+108h],0 ; je ; mov eax,ebx ;
/// call release ; (drop lists) ; cmp byte [ebx+109h],0 ; jne hold_new
const ASSIGN_IDA: &str = "55 57 56 53 83 EC 0C A1 ?? ?? ?? ?? 8B 5C 24 20 8B 74 24 24 3B 70 44 0F 83 ?? ?? ?? ?? \
                          8B 40 40 8B 2C B0 B8 FF FF 00 00 85 ED 0F 44 F0 80 BB 08 01 00 00 00 74 07 89 D8 E8 ?? ?? ?? \
                          ?? 83 EC 0C 53 E8 ?? ?? ?? ?? 83 C4 10 80 BB 09 01 00 00 00 0F 85";
const ASSIGN_RELEASE: usize = 0x38;
/// Its hold of the new appearance: sub esp,0Ch ; mov edi,[ebx+100h] ; mov [ebx+100h],esi ; push esi ; call hold
const HOLD: &str = "83 EC 0C 8B BB 00 01 00 00 89 B3 00 01 00 00 56 E8";
const HOLD_CALL: usize = 16;
/// The release: push esi ; push ebx ; mov ebx,eax ; sub esp,10h ; push eax ; call drop_lists ; pop eax ;
/// mov byte [ebx+108h],0 ; push [ebx+1Ch] ; call string_release
const RELEASE_IDA: &str = "56 53 89 C3 83 EC 10 50 E8 ?? ?? ?? ?? 58 C6 83 08 01 00 00 00 FF 73 1C E8";
const RELEASE_STRING: usize = 0x18;
/// The take (its callers check the copy doesn't hold its own references): push esi ; push ebx ; mov ebx,eax ;
/// sub esp,20h ; mov byte [eax+108h],1 ; push [eax+1Ch] ; call string_add
const TAKE_IDA: &str = "56 53 89 C3 83 EC 20 C6 80 08 01 00 00 01 FF 70 1C E8";
const TAKE_STRING: usize = 0x11;
/// The three start with 7 bytes of whole position-independent instructions.
const ENTRY: usize = 7;
/// The copy's var write of render_source and render_target (ebx = the copy): push 6 ; push [ebp-44h] ;
/// push [ebp-48h] ; call to_string ; add esp,10h ; mov [ebx+field],eax ; jmp done
const SOURCE_WRITE: &str = "6A 06 FF 75 BC FF 75 B8 E8 ?? ?? ?? ?? 83 C4 10 89 83 BC 00 00 00 E9";
const TARGET_WRITE: &str = "6A 06 FF 75 BC FF 75 B8 E8 ?? ?? ?? ?? 83 C4 10 89 83 C0 00 00 00 E9";
const WRITE_CALL: usize = 8;
/// A new mutable appearance's first publish, inlined in image creation: (copy's publish count + 1) ;
/// mov [reg+0E8h],edx ; test eax,eax ; jne done ; sub esp,0Ch ; push [reg+104h] ; call hold
const PUBLISH: &str = "89 ?? E8 00 00 00 85 C0 0F 85 ?? ?? ?? ?? 83 EC 0C FF ?? 04 01 00 00 E8";
const PUBLISH_HOLD: usize = 0x17;

/// The appearance assignment, then [`assigned`]. It ends in a tail call that reuses its argument slots, so the copy
/// is kept here.
unsafe extern "C" fn assign(copy: usize, id: u32) {
    // SAFETY: BYOND's call, passed on, on its working copy.
    unsafe {
        ASSIGN.get()(copy, id);
        assigned(copy);
    }
}

/// The take, then [`took`]. 16-byte stack alignment at each call.
#[unsafe(naked)]
unsafe extern "C" fn take_stub() {
    naked_asm!(
        "sub esp, 8",
        "push eax",
        "call dword ptr [{take}]",
        "call {took}",
        "add esp, 12",
        "ret",
        take = sym TAKE,
        took = sym took,
    )
}

/// [`releasing`], then the release.
#[unsafe(naked)]
unsafe extern "C" fn release_stub() {
    naked_asm!(
        "sub esp, 8",
        "push eax",
        "call {releasing}",
        "pop eax",
        "add esp, 8",
        "jmp dword ptr [{release}]",
        releasing = sym releasing,
        release = sym RELEASE,
    )
}

render_stub!(source_stub, super::set_source, "ebx");
render_stub!(target_stub, super::set_target, "ebx");

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let assign_fn = core.find_unique(ASSIGN_IDA, "appearance assignment")?;
    let release_fn = core.follow_call(assign_fn + ASSIGN_RELEASE, RELEASE_IDA, "appearance assignment: release")?;
    let hold =
        core.call_target(core.find_unique_in(assign_fn, 0x200, HOLD, "appearance assignment: hold")? + HOLD_CALL)?;
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
            add: Foreign::at(core.call_target(take_fn + TAKE_STRING)?),
            release: Foreign::at(core.call_target(release_fn + RELEASE_STRING)?),
        },
    );
    TO_STRING.set(to_string);
    ASSIGN.set(core.hook(assign_fn, ENTRY, assign as Assign)?);
    TAKE.set(core.hook(take_fn, ENTRY, take_stub as unsafe extern "C" fn())?);
    RELEASE.set(core.hook(release_fn, ENTRY, release_stub as unsafe extern "C" fn())?);
    core.retarget(source + WRITE_CALL + 1, source_stub as unsafe extern "C" fn())?;
    core.retarget(target + WRITE_CALL + 1, target_stub as unsafe extern "C" fn())?;
    core.write_code(publish + PUBLISH_HOLD, &super::NO_CALL)?;
    Ok(core.report(&[assign_fn, take_fn, release_fn]))
}
