//! ucrtbase.dll (the Universal C Runtime): resolving its functions ([`Image::ucrt_slot`], [`ucrt_export`]), and the
//! `FILE` layout check shared by the stream fast paths (features `stdio`, `dmbread`).

use std::ffi::c_void;
use std::sync::LazyLock;

use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

use crate::code::{Foreign, live_u32};
use crate::image::Image;
use crate::windows::mem::module_range;
use crate::{Result, bail};

impl Image {
    /// [`Image::iat_slot`], checked to resolve into ucrtbase.dll (hooks on the module's `FILE`s assume its layout).
    pub fn ucrt_slot(&self, name: &str) -> Result<usize> {
        let slot = self.iat_slot(name)?;
        let (start, end) = module_range("ucrtbase.dll").ok_or("ucrtbase.dll not loaded")?;
        if !(start..end).contains(&(self.ptr_at(slot)?)) {
            bail!("{name} is not ucrtbase's");
        }
        Ok(slot)
    }
}

/// ucrtbase.dll's export `name`, for calls on byondcore's streams, errno or heap blocks (dmfast's own CRT is not
/// ucrtbase).
pub fn ucrt_export(name: &str) -> Result<usize> {
    let (start, end) = module_range("ucrtbase.dll").ok_or("ucrtbase.dll not loaded")?;
    let c: Vec<u8> = name.bytes().chain([0]).collect();
    // SAFETY: `start` is ucrtbase.dll's handle; `c` is NUL-terminated.
    let f = unsafe { GetProcAddress(start as _, c.as_ptr()) }.map_or(0, |f| f as usize);
    if f == 0 {
        bail!("ucrtbase.dll exports no {name}");
    }
    if !(start..end).contains(&f) {
        bail!("{name} is not ucrtbase's");
    }
    Ok(f)
}

/// Checks the undocumented UCRT `FILE` fields the stream fast paths use (`_ptr` +0, `_cnt` +8, read/write flags
/// in `_flags` +0Ch) after one `fputc` and one `getc` on a pipe. Run once; another layout turns those patches off.
pub fn ucrt_file_layout() -> Result<()> {
    static RESULT: LazyLock<Result<()>> =
        LazyLock::new(|| probe().map_err(|e| format!("UCRT FILE layout: {e}").into()));
    RESULT.clone()
}

fn probe() -> Result<()> {
    const O_BINARY: i32 = 0x8000;
    const BUF: usize = 64;
    macro_rules! ucrt {
        ($name:ident: $ty:ty) => {
            let $name = Foreign::<$ty>::at(ucrt_export(stringify!($name))?);
        };
    }
    ucrt!(_pipe: unsafe extern "C" fn(*mut i32, u32, i32) -> i32);
    ucrt!(_fdopen: unsafe extern "C" fn(i32, *const u8) -> *mut c_void);
    ucrt!(_close: unsafe extern "C" fn(i32) -> i32);
    ucrt!(_write: unsafe extern "C" fn(i32, *const u8, u32) -> i32);
    ucrt!(setvbuf: unsafe extern "C" fn(*mut c_void, *mut u8, i32, usize) -> i32);
    ucrt!(fputc: unsafe extern "C" fn(i32, *mut c_void) -> i32);
    ucrt!(getc: unsafe extern "C" fn(*mut c_void) -> i32);
    ucrt!(fclose: unsafe extern "C" fn(*mut c_void) -> i32);
    // SAFETY: ucrtbase's exports have these signatures; they get a pipe, streams and buffers of this function's own.
    unsafe {
        let (_pipe, _fdopen, _close, _write) = (_pipe.get(), _fdopen.get(), _close.get(), _write.get());
        let (setvbuf, fputc, getc, fclose) = (setvbuf.get(), fputc.get(), getc.get(), fclose.get());
        let mut fds = [0i32; 2];
        if _pipe(fds.as_mut_ptr(), 256, O_BINARY) != 0 {
            return Err("no pipe".into());
        }
        if _write(fds[1], b"ab".as_ptr(), 2) != 2 {
            _close(fds[0]);
            _close(fds[1]);
            return Err("pipe write failed".into());
        }
        let (r, w) = (_fdopen(fds[0], c"rb".as_ptr().cast()), _fdopen(fds[1], c"wb".as_ptr().cast()));
        if r.is_null() || w.is_null() {
            let close = |s: *mut c_void, fd| if s.is_null() { _close(fd) } else { fclose(s) };
            close(r, fds[0]);
            close(w, fds[1]);
            return Err("_fdopen failed".into());
        }
        let (mut rbuf, mut wbuf) = ([0u8; BUF], [0u8; BUF]);
        setvbuf(r, rbuf.as_mut_ptr(), 0, BUF);
        setvbuf(w, wbuf.as_mut_ptr(), 0, BUF);
        let field = |f: *mut c_void, off: usize| live_u32(f as usize + off);
        let read_ok = getc(r) == b'a' as i32
            && field(r, 0) as usize == rbuf.as_ptr() as usize + 1
            && field(r, 8) == 1
            && field(r, 0xC) & 3 == 1;
        let write_ok = fputc(b'x' as i32, w) == b'x' as i32
            && field(w, 0) as usize == wbuf.as_ptr() as usize + 1
            && field(w, 8) as usize == BUF - 1
            && wbuf[0] == b'x'
            && field(w, 0xC) & 3 == 2;
        fclose(w);
        fclose(r);
        match (read_ok, write_ok) {
            (true, true) => Ok(()),
            (false, _) => Err("getc left _ptr/_cnt/_flags elsewhere".into()),
            (_, false) => Err("fputc left _ptr/_cnt/_flags elsewhere".into()),
        }
    }
}
