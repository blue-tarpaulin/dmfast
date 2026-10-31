//! `dmbbuf`, Windows: byondcore's `_wfopen` import calls [`wfopen`].

use std::sync::atomic::Ordering::Relaxed;

use libc::FILE;

use super::{BUFFER, BUFFERED, is_dmb};
use crate::base::Result;
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::windows::ucrt::ucrt_export;

/// The `_wfopen` byondcore imported (or `fscache`'s hook on it).
static WFOPEN: Foreign<unsafe extern "C" fn(*const u16, *const u16) -> *mut FILE> = Foreign::new();
/// ucrtbase's `setvbuf`: byondcore's streams are ucrtbase's.
static SETVBUF: Foreign<unsafe extern "C" fn(*mut FILE, *mut i8, i32, usize) -> i32> = Foreign::new();

/// `_wfopen`, then the buffer for a `.dmb`.
///
/// # Safety
/// `name` and `mode` are NUL-terminated wide strings (byondcore's `_wfopen` call).
unsafe extern "C" fn wfopen(name: *const u16, mode: *const u16) -> *mut FILE {
    // SAFETY: NUL-terminated (caller); WFOPEN and SETVBUF were set at install; `f` is the stream just opened.
    unsafe {
        let f = WFOPEN.get()(name, mode);
        if !f.is_null() && !name.is_null() {
            let len = (0..).take_while(|&i| *name.add(i) != 0).count();
            if is_dmb(std::slice::from_raw_parts(name, len))
                && SETVBUF.get()(f, std::ptr::null_mut(), libc::_IOFBF, BUFFER) == 0
            {
                BUFFERED.fetch_add(1, Relaxed);
            }
        }
        f
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // ucrtbase's setvbuf needs ucrtbase's streams.
    core.ucrt_slot("fclose")?;
    SETVBUF.set(ucrt_export("setvbuf")?);
    let open = core.iat_slot("_wfopen")?;
    WFOPEN.set(core.live_u32(open)? as usize);
    core.wrap_data(open, &(wfopen as *const () as u32).to_le_bytes())?;
    Ok(format!("{} KB .dmb buffer", BUFFER / 1024))
}
