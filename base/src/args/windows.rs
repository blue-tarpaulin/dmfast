//! Windows: the command line, which the program splits into arguments after `DllMain`; its wide and ANSI copies are
//! edited in place.

use windows_sys::Win32::System::Environment::{GetCommandLineA, GetCommandLineW};

pub(super) unsafe fn remove(flag: &str) {
    // SAFETY: the process's command lines, writable and NUL-terminated; install time, so no one else uses them
    // (caller).
    unsafe {
        remove_from(GetCommandLineW().cast_mut(), flag);
        remove_from(GetCommandLineA().cast_mut(), flag);
    }
}

/// Cut each `flag` outside quotes that stands alone between blanks, with the blank before it, out of `line`.
///
/// # Safety
/// `line` is a writable NUL-terminated string no one else uses meanwhile.
unsafe fn remove_from<T: Copy + Into<u32>>(line: *mut T, flag: &str) {
    // SAFETY: NUL-terminated (caller); the slice takes the NUL too.
    let line = unsafe {
        let len = (0..).take_while(|&i| (*line.add(i)).into() != 0).count();
        std::slice::from_raw_parts_mut(line, len + 1)
    };
    let blank = |u: u32| matches!(u, 0x20 | 0x09 | 0);
    let flag = flag.as_bytes();
    let (mut i, mut quoted, mut end) = (0, false, line.len());
    while i + 1 + flag.len() < end {
        let at = &line[i + 1..i + 1 + flag.len()];
        let found = !quoted
            && blank(line[i].into())
            && blank(line[i + 1 + flag.len()].into())
            && at.iter().zip(flag).all(|(&u, f)| u8::try_from(u.into()).is_ok_and(|u| u.eq_ignore_ascii_case(f)));
        if found {
            line.copy_within(i + 1 + flag.len()..end, i);
            end -= 1 + flag.len();
        } else {
            quoted ^= line[i].into() == u32::from(b'"');
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removed(line: &str) -> String {
        let mut buf: Vec<u16> = line.encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated buffer only this test uses.
        unsafe { remove_from(buf.as_mut_ptr(), "--offline") };
        String::from_utf16_lossy(&buf[..buf.iter().position(|&u| u == 0).unwrap()])
    }

    #[test]
    fn removes_whole_unquoted_arguments() {
        assert_eq!(removed("dd.exe w.dmb 5000 --offline -trusted"), "dd.exe w.dmb 5000 -trusted");
        assert_eq!(removed("dd.exe --OFFLINE\t--offline x"), "dd.exe x");
        let kept = r#""C:\by --offline\dd.exe" "a --offline b" --offlinex x--offline"#;
        assert_eq!(removed(kept), kept);
    }
}
