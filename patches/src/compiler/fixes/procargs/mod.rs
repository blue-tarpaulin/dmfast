//! `procargs`: two argument-list mistakes BYOND lets through become compile errors.
//!
//! - Duplicate argument names (forum report 2737322) compiled without an error, the last one winning. The argument
//!   compile pass is hooked: before it runs, a name repeated among an argument list's `var` children is reported.
//! - An argument starting with `/var/` (`proc/foo(/var/bar = 5)`, forum report 2830750) became a global var, and the
//!   proc lost the argument. The type tree insertion is hooked: a path starting with `/` then `var` below an
//!   argument list is reported. Other typed arguments (`/mob/target`) are left alone; `/global`, `/static`, `/const`,
//!   and `/tmp` already fail.
//!
//! Both only report, through BYOND's own compile error function, and then run the stock code: a project without
//! these mistakes compiles as before.

use std::ffi::{CStr, c_char};

use crate::base::code::{Foreign, read};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;
use crate::compiler::layout::{CHILDREN, COUNT, ID};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static PROCARGS: Feature = Feature::fix(
    Hosts::COMPILER,
    "procargs",
    "proc argument list errors",
    Tier::Extended,
    install!(windows: windows::install, linux: linux::install),
);

/// An argument node's tag byte, [`VAR`] for a `var` (pinned by each platform's signature).
const TAG: usize = 0x10;
const VAR: u8 = 0x11;
/// A `var` argument's name id (u32), pinned by each platform's signature.
const NAME: usize = 0xC;
const DUPLICATE: &CStr = c"duplicate argument name";
const ABSOLUTE: &CStr = c"argument cannot start with /";

/// BYOND's compile error function (cdecl: node, message).
type ErrorFn = unsafe extern "C" fn(usize, *const c_char);

/// What install found: the error function and the ids the checks compare, which change between builds.
struct Found {
    error: Foreign<ErrorFn>,
    /// An argument list's node id.
    arg_list: u32,
    /// The path token kinds of `var` and `/`.
    var: u32,
    slash: u32,
}

static FOUND: Installed<Found> = Installed::new();

/// Whether `node` is a `var` argument.
///
/// # Safety
/// `node` is a live node of the compiler's tree.
unsafe fn is_var(node: usize) -> bool {
    // SAFETY: a live node (caller).
    unsafe { read::<u8>(node + TAG) == VAR }
}

/// Reports each `var` child of `node` whose name an earlier one has.
///
/// # Safety
/// `node` is a live node of the compiler's tree.
unsafe fn report_duplicates(node: usize) {
    // SAFETY: a live node, its child array, and its children (caller); the error function was found at install.
    unsafe {
        let kids = read::<usize>(node + CHILDREN) as *const usize;
        let var = |i: usize| {
            let kid = *kids.add(i);
            is_var(kid).then(|| (kid, read::<u32>(kid + NAME)))
        };
        for j in 1..usize::from(read::<u16>(node + COUNT)) {
            let Some((arg, name)) = var(j) else { continue };
            if (0..j).any(|i| var(i).is_some_and(|(_, earlier)| earlier == name)) {
                FOUND.get().error.get()(arg, DUPLICATE.as_ptr());
            }
        }
    }
}

/// Reports a path starting with `/var/` inserted below an argument list: `path` is the insertion's token kind
/// vector (pointer, capacity, length), `parent` its parent node or null. `extern "C"` for the Linux entry stub.
///
/// # Safety
/// `path` is a live vector, `parent` a live node or null.
unsafe extern "C" fn check_path(path: usize, parent: usize) {
    // SAFETY: a live vector and node (caller); the error function was found at install.
    unsafe {
        let found = FOUND.get();
        if parent != 0 && read::<u32>(parent + ID) == found.arg_list && read::<usize>(path + 8) > 1 {
            let kinds = read::<usize>(path);
            if read::<u32>(kinds) == found.slash && read::<u32>(kinds + 4) == found.var {
                found.error.get()(parent, ABSOLUTE.as_ptr());
            }
        }
    }
}
