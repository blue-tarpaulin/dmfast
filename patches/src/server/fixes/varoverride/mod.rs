//! `varoverride` (forum report 2992603): in a project with about 65,500 names, a child type's override of a var its
//! parent sets at runtime (`var/list/x = list(1)` in the parent, `x = null` in the child) is no longer ignored.
//!
//! At load, each type's init proc is combined with its parent's: running an init proc in a recording mode notes the
//! code that sets each var, keyed by the var's name, and the parent's code for the vars the child overrides is left
//! out. Bytecode names a var by its string id, except that the compiler writes an id among the interpreter's built-in
//! ids (0xFFCD-0xFFF1) with a flag in the top byte (0x10000000), which the interpreter strips (`and 0FFFFFFh`). The
//! recording keeps the flag while the child's overrides use the plain id, so those overrides never match. The fix
//! strips the flag in the recording. It's safe because every other string id is below 2^24 already, so the plain id
//! is what the recording would have stored without the flag.

use crate::base::code::Foreign;
use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static VAROVERRIDE: Feature = Feature::fix(
    Hosts::SERVER,
    "varoverride",
    "child types' var overrides apply in projects with ~65,500 names",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// The recording of one var's init code: (var name, init proc, end of its code), cdecl.
type RecordFn = unsafe extern "C" fn(u32, u32, u32);

/// The bits of a var name operand that hold the string id.
const STRING_ID: u32 = 0x00FF_FFFF;

/// The stock recorder, which the hook calls.
static RECORD: Foreign<RecordFn> = Foreign::new();

/// The recorder with the plain string id.
extern "C" fn record(name: u32, proc_id: u32, end: u32) {
    // SAFETY: the stock recorder, found at install; the arguments are BYOND's own.
    unsafe { RECORD.get()(name & STRING_ID, proc_id, end) }
}
