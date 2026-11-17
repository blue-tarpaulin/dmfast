//! `dmbbuf`, Linux: libbyond's `fopen` and `fopen64` call sites call [`fopen`] and [`fopen64`].

use std::ffi::{CStr, c_char};

use libc::FILE;

use super::{BUFFER, BUFFERED, is_dmb};
use crate::base::code::branch;
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

type Fopen = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut FILE;

/// The buffer for `f` if it is a `.dmb`; `f`.
///
/// # Safety
/// `name` is null or a C string; `f` is null or the stream just opened from it.
unsafe fn buffered(name: *const c_char, f: *mut FILE) -> *mut FILE {
    // SAFETY: a C string and its open stream (caller).
    unsafe {
        if !f.is_null()
            && !name.is_null()
            && is_dmb(CStr::from_ptr(name).to_bytes())
            && libc::setvbuf(f, std::ptr::null_mut(), libc::_IOFBF, BUFFER) == 0
        {
            BUFFERED.bump();
        }
    }
    f
}

/// # Safety
/// As libc's `fopen`.
unsafe extern "C" fn fopen(name: *const c_char, mode: *const c_char) -> *mut FILE {
    // SAFETY: libc's fopen with the caller's arguments.
    unsafe { buffered(name, libc::fopen(name, mode)) }
}

/// # Safety
/// As libc's `fopen64`.
unsafe extern "C" fn fopen64(name: *const c_char, mode: *const c_char) -> *mut FILE {
    // SAFETY: libc's fopen64 with the caller's arguments.
    unsafe { buffered(name, libc::fopen64(name, mode)) }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut sites = Vec::new();
    for (name, libc_fn, ours) in
        [("fopen", libc::fopen as Fopen, fopen as Fopen), ("fopen64", libc::fopen64 as Fopen, fopen64 as Fopen)]
    {
        for s in core.import_calls(name, libc_fn)? {
            sites.push((s, ours));
        }
    }
    ensure!(!sites.is_empty(), "no fopen or fopen64 call sites");
    for &(s, ours) in &sites {
        // A call or a tail jump: keep the opcode.
        let op = core.u8_at(s)?;
        core.write_code(s, &branch(op, s, ours, 5))?;
    }
    Ok(format!("{} KB .dmb buffer, {} fopen sites", BUFFER / 1024, sites.len()))
}
