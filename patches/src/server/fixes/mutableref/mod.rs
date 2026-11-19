//! `mutableref` (forum reports 2992947, 2992948 and 2992949): a `/mutable_appearance` keeps the strings and lists it
//! shows alive.
//!
//! A mutable appearance's working copy can borrow another appearance's fields (its strings and overlay lists)
//! without its own references, as long as it holds a reference on that appearance. `ma.appearance = X` borrows X's
//! fields, but the var write it goes through first lets go of the old appearance, so BYOND's branch that would hold
//! X never runs: once nothing else uses X, its strings and lists are freed and reused, and `ma` shows another
//! thing's icon state, overlays, and name, or ends in "Bad ref" messages and a crash. The fix holds X after the
//! assignment, as BYOND does when it makes a mutable appearance from an appearance.
//!
//! Separately, `render_target` and `render_source` on a working copy were stored with no reference, so a string
//! made at runtime (`"*x_[id]"`) was freed after the statement. They now count like the copy's other strings: held
//! while the copy holds its own references, taken and let go with the rest.
//!
//! And every mutable appearance made from an appearance (`new /mutable_appearance(X)`) leaked one reference on X,
//! keeping it and its strings forever: the copy's constructor holds X for the copy's base slot, and its first
//! publish holds that slot again, which nothing lets go of (the copy's publish count is never lowered). The publish's
//! second hold is dropped; the var write that lets go of the base slot, and the destructor, then balance.
//!
//! Safe: the references added are each let go by BYOND's own code (the copy's destructor) or by the fix with the
//! copy's other strings, and the one dropped was never let go.

use crate::base::code::{self, Foreign};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static MUTABLEREF: Feature = Feature::fix(
    Hosts::SERVER,
    "mutableref",
    "a mutable appearance holds what it shows, and doesn't leak the appearance it was made from",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// Working copy offsets: the appearance it shows, whether it holds its own references to its fields, and whether it
/// holds a reference on the appearance.
#[cfg(windows)]
const SHOWN: usize = 0x104;
#[cfg(target_os = "linux")]
const SHOWN: usize = 0x100;
const OWNS: usize = SHOWN + 8;
const HOLDS: usize = SHOWN + 9;
/// `render_source` and `render_target`: string ids.
const RENDER_SOURCE: usize = 0xBC;
const RENDER_TARGET: usize = 0xC0;
/// No appearance.
const NONE: u32 = 0xFFFF;
/// Replaces a `call`: the publish's second hold.
const NO_CALL: [u8; 5] = [0x90; 5];

/// BYOND's reference add on an appearance, and add and release on a string (cdecl, the id).
struct Found {
    hold: Foreign<unsafe extern "C" fn(u32)>,
    add: Foreign<unsafe extern "C" fn(u32)>,
    release: Foreign<unsafe extern "C" fn(u32)>,
}

static FOUND: Installed<Found> = Installed::new();

/// After an appearance assignment: hold the appearance the copy now shows, if BYOND didn't.
///
/// # Safety
/// `copy` is a working copy.
unsafe fn assigned(copy: usize) {
    // SAFETY: the copy's fields (caller); BYOND's reference add on a live appearance.
    unsafe {
        let shown = code::read::<u32>(copy + SHOWN);
        if shown != NONE && code::read::<u8>(copy + HOLDS) == 0 {
            FOUND.get().hold.get()(shown);
            code::write::<u8>(copy + HOLDS, 1);
        }
    }
}

/// Whether the copy holds its own references.
///
/// # Safety
/// `copy` is a working copy.
unsafe fn owns(copy: usize) -> bool {
    // SAFETY: the copy's flag (caller).
    unsafe { code::read::<u8>(copy + OWNS) != 0 }
}

/// Applies BYOND's string add or release to the copy's render strings, if it holds its own references.
///
/// # Safety
/// `copy` is a working copy whose render strings are live.
unsafe fn render_refs(copy: usize, f: &Foreign<unsafe extern "C" fn(u32)>) {
    // SAFETY: the copy's fields (caller); BYOND's string add or release.
    unsafe {
        if owns(copy) {
            let f = f.get();
            f(code::read::<u32>(copy + RENDER_SOURCE));
            f(code::read::<u32>(copy + RENDER_TARGET));
        }
    }
}

/// After a take that gave the copy its own references: add one on its render strings too.
extern "C" fn took(copy: usize) {
    // SAFETY: the take's working copy.
    unsafe { render_refs(copy, &FOUND.get().add) }
}

/// Before the copy's release of its own references: let go of its render strings.
extern "C" fn releasing(copy: usize) {
    // SAFETY: the release's working copy.
    unsafe { render_refs(copy, &FOUND.get().release) }
}

/// A render string var write on the copy, after the value's conversion to a string id: counted while the copy
/// holds its own references. Returns `id` for BYOND's own store that follows.
///
/// # Safety
/// `copy` is a working copy and `field` one of its render string offsets.
unsafe fn set_render(copy: usize, field: usize, id: u32) -> u32 {
    // SAFETY: the copy's field (caller); BYOND's string add and release.
    unsafe {
        if owns(copy) {
            let found = FOUND.get();
            found.add.get()(id);
            found.release.get()(code::read::<u32>(copy + field));
        }
        code::write::<u32>(copy + field, id);
    }
    id
}

extern "C" fn set_source(copy: usize, id: u32) -> u32 {
    // SAFETY: the var write's working copy.
    unsafe { set_render(copy, RENDER_SOURCE, id) }
}

extern "C" fn set_target(copy: usize, id: u32) -> u32 {
    // SAFETY: the var write's working copy.
    unsafe { set_render(copy, RENDER_TARGET, id) }
}

/// The conversion of a value to a string id (cdecl: its type, its data, 6).
type ToString = unsafe extern "C" fn(u32, u32, u32) -> u32;

static TO_STRING: Foreign<ToString> = Foreign::new();

/// In place of a render var write's conversion, with the copy in `$copy`: the conversion, then `$set`, which returns
/// the id for BYOND's store that follows. Keeps 16-byte stack alignment at each call.
macro_rules! render_stub {
    ($name:ident, $set:path, $copy:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            std::arch::naked_asm!(
                "push dword ptr [esp + 12]",
                "push dword ptr [esp + 12]",
                "push dword ptr [esp + 12]",
                "call dword ptr [{to_string}]",
                "add esp, 12",
                "sub esp, 4",
                "push eax",
                concat!("push ", $copy),
                "call {set}",
                "add esp, 12",
                "ret",
                to_string = sym super::TO_STRING,
                set = sym $set,
            )
        }
    };
}
use render_stub;
