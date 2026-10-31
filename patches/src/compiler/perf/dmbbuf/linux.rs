//! `dmbbuf`, Linux: libbyond's `fopen` call sites call [`fopen`].

use std::ffi::{CStr, c_char};
use std::sync::atomic::Ordering::Relaxed;

use libc::FILE;

use super::{BUFFER, BUFFERED, is_dmb};
use crate::base::code::branch;
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

type Fopen = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut FILE;

/// `fopen`, then the buffer for a `.dmb`.
///
/// # Safety
/// `name` and `mode` are C strings (libbyond's `fopen` call).
unsafe extern "C" fn fopen(name: *const c_char, mode: *const c_char) -> *mut FILE {
    // SAFETY: C strings (caller); `f` is the stream just opened.
    unsafe {
        let f = libc::fopen(name, mode);
        if !f.is_null()
            && !name.is_null()
            && is_dmb(CStr::from_ptr(name).to_bytes())
            && libc::setvbuf(f, std::ptr::null_mut(), libc::_IOFBF, BUFFER) == 0
        {
            BUFFERED.fetch_add(1, Relaxed);
        }
        f
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let sites = core.import_calls("fopen", libc::fopen as Fopen as usize)?;
    if sites.is_empty() {
        bail!("no fopen call sites");
    }
    for &s in &sites {
        // A call or a tail jump: keep the opcode.
        let [op] = core.array(s)?;
        core.write_code(s, &branch(op, s, fopen as Fopen as usize, 5))?;
    }
    Ok(format!("{} KB .dmb buffer, {} fopen sites", BUFFER / 1024, sites.len()))
}
