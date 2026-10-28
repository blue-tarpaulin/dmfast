//! Calls that can modify the file system, watched through the import tables of dm.exe, byondcore, and byondext.
//! Each records the paths it touches, then calls the original.

use std::ffi::c_void;
use std::sync::atomic::Ordering::Relaxed;

use libc::{O_APPEND, O_CREAT, O_RDWR, O_TRUNC, O_WRONLY};
use windows_sys::Win32::Foundation::{GENERIC_ALL, GENERIC_WRITE, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_FLAG_DELETE_ON_CLOSE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
    OPEN_EXISTING, WRITE_DAC, WRITE_OWNER,
};

use super::{CWD_GEN, OFF, touched_a, touched_w};
use crate::base::code::Foreign;

/// A watched import: the module importing it, its name, our hook, and where the original goes.
pub(super) struct Modifier {
    pub module: &'static str,
    pub import: &'static str,
    pub hook: usize,
    pub set_original: fn(usize),
}

/// The module name used for the program (dm.exe), whatever its file is called.
pub(super) const HOST: &str = "dm.exe";

/// Each row: `(module, import) abi fn(args) -> ret { before the call } [then { after it }]`. The hook runs `before`,
/// calls the original with the same arguments, runs `then`, and returns the original's result.
macro_rules! modifiers {
    ($(
        ($module:expr, $import:literal) $abi:literal fn($($a:ident: $t:ty),*) -> $r:ty { $($before:tt)* }
            $(then { $($after:tt)* })?
    )*) => {
        /// Every watched import.
        pub(super) fn modifiers() -> Vec<Modifier> {
            vec![$({
                static ORIGINAL: Foreign<unsafe extern $abi fn($($t),*) -> $r> = Foreign::new();
                unsafe extern $abi fn hook($($a: $t),*) -> $r {
                    // SAFETY: the original import, called as the module would have; the paths are the module's.
                    unsafe {
                        $($before)*
                        let result = ORIGINAL.get()($($a),*);
                        $($($after)*)?
                        result
                    }
                }
                Modifier {
                    module: $module,
                    import: $import,
                    hook: hook as *const () as usize,
                    set_original: |a| ORIGINAL.set(a),
                }
            }),*]
        }
    };
}

modifiers! {
    ("byondcore.dll", "CreateFileW") "system"
        fn(n: *const c_void, a: u32, s: u32, sa: *const c_void, d: u32, f: u32, t: HANDLE) -> HANDLE {
            if create_modifies(a, d, f) {
                touched_w(n.cast());
            }
        }
    ("byondcore.dll", "CreateFileA") "system"
        fn(n: *const c_void, a: u32, s: u32, sa: *const c_void, d: u32, f: u32, t: HANDLE) -> HANDLE {
            if create_modifies(a, d, f) {
                touched_a(n.cast());
            }
        }
    ("byondcore.dll", "SetFileAttributesW") "system" fn(n: *const u16, a: u32) -> i32 { touched_w(n); }
    ("byondcore.dll", "DeleteFileA") "system" fn(n: *const u8) -> i32 { touched_a(n); }
    ("byondcore.dll", "DeleteFileW") "system" fn(n: *const u16) -> i32 { touched_w(n); }
    // Arbitrary operations on arbitrary paths: cache off for good.
    ("byondcore.dll", "SHFileOperationW") "system" fn(op: *mut c_void) -> i32 { OFF.store(true, Relaxed); }
    ("byondcore.dll", "_wfopen") "C" fn(n: *const u16, mode: *const u16) -> *mut c_void {
        if mode_modifies(mode) {
            touched_w(n);
        }
    }
    ("byondcore.dll", "fopen") "C" fn(n: *const u8, mode: *const u8) -> *mut c_void {
        if mode_modifies(mode) {
            touched_a(n);
        }
    }
    ("byondcore.dll", "_wremove") "C" fn(n: *const u16) -> i32 { touched_w(n); }
    ("byondcore.dll", "remove") "C" fn(n: *const u8) -> i32 { touched_a(n); }
    ("byondcore.dll", "_wmkdir") "C" fn(n: *const u16) -> i32 { touched_w(n); }
    ("byondcore.dll", "_wrmdir") "C" fn(n: *const u16) -> i32 { touched_w(n); }
    ("byondcore.dll", "_wrename") "C" fn(a: *const u16, b: *const u16) -> i32 {
        touched_w(a);
        touched_w(b);
    }
    // Changes what relative paths mean: the current directory is re-read before the next lookup.
    ("byondcore.dll", "_wchdir") "C" fn(n: *const u16) -> i32 {} then { CWD_GEN.fetch_add(1, Relaxed); }
    ("byondext.dll", "fopen") "C" fn(n: *const u8, mode: *const u8) -> *mut c_void {
        if mode_modifies(mode) {
            touched_a(n);
        }
    }
    // `_open(path, flags, ...)`: the optional mode is passed on as a third word, which is harmless in cdecl.
    ("byondext.dll", "_open") "C" fn(n: *const u8, flags: i32, pmode: i32) -> i32 {
        if flags & (O_WRONLY | O_RDWR | O_APPEND | O_CREAT | O_TRUNC) != 0 {
            touched_a(n);
        }
    }
    ("byondcore.dll", "_wfsopen") "C" fn(n: *const u16, mode: *const u16, share: i32) -> *mut c_void {
        if mode_modifies(mode) {
            touched_w(n);
        }
    }
    ("byondcore.dll", "_wunlink") "C" fn(n: *const u16) -> i32 { touched_w(n); }
    // File times change what GetFileAttributesExW and _wstat64i32 answer.
    ("byondcore.dll", "_utime64") "C" fn(n: *const u8, times: *mut c_void) -> i32 { touched_a(n); }
    // dm.exe changes to the .dme's directory after ByondLib::Init.
    (HOST, "_chdir") "C" fn(n: *const u8) -> i32 {} then { CWD_GEN.fetch_add(1, Relaxed); }
}

/// The access rights that let a handle change a file.
const WRITE_RIGHTS: u32 = GENERIC_WRITE
    | GENERIC_ALL
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER;

fn create_modifies(access: u32, disposition: u32, flags: u32) -> bool {
    access & WRITE_RIGHTS != 0 || disposition != OPEN_EXISTING || flags & FILE_FLAG_DELETE_ON_CLOSE != 0
}

/// Whether an fopen-style mode string has `w`, `a`, or `+`.
unsafe fn mode_modifies<T: Copy + Into<u32>>(mut p: *const T) -> bool {
    unsafe {
        while (*p).into() != 0 {
            if matches!(char::from_u32((*p).into()), Some('w' | 'a' | '+')) {
                return true;
            }
            p = p.add(1);
        }
    }
    false
}

/// Every import that can change what the cached calls answer, plus `FindNextFile*`, which would get the fake find
/// handles. A watched module that imports one must watch it too, or the cache stays off. Not listed: handle-based
/// calls (the write-mode open is watched) and `tmpfile` (a new unique file).
pub(super) const FILE_SYSTEM: &[&str] = &[
    "CreateFileW",
    "CreateFileA",
    "CreateFile2",
    "SetFileAttributesW",
    "SetFileAttributesA",
    "DeleteFileW",
    "DeleteFileA",
    "MoveFileW",
    "MoveFileA",
    "MoveFileExW",
    "MoveFileExA",
    "MoveFileWithProgressW",
    "MoveFileWithProgressA",
    "CopyFileW",
    "CopyFileA",
    "CopyFileExW",
    "CopyFileExA",
    "CopyFile2",
    "ReplaceFileW",
    "ReplaceFileA",
    "CreateDirectoryW",
    "CreateDirectoryA",
    "CreateDirectoryExW",
    "CreateDirectoryExA",
    "RemoveDirectoryW",
    "RemoveDirectoryA",
    "CreateHardLinkW",
    "CreateHardLinkA",
    "CreateSymbolicLinkW",
    "CreateSymbolicLinkA",
    "SetCurrentDirectoryW",
    "SetCurrentDirectoryA",
    "SHFileOperationW",
    "SHFileOperationA",
    "FindNextFileW",
    "FindNextFileA",
    "_wfopen",
    "fopen",
    "_wfopen_s",
    "fopen_s",
    "_wfsopen",
    "_fsopen",
    "_wfreopen",
    "freopen",
    "_wfreopen_s",
    "freopen_s",
    "_wopen",
    "_open",
    "_wsopen",
    "_sopen",
    "_wsopen_s",
    "_sopen_s",
    "_wcreat",
    "_creat",
    "_wremove",
    "remove",
    "_wunlink",
    "_unlink",
    "_wrename",
    "rename",
    "_wmkdir",
    "_mkdir",
    "_wrmdir",
    "_rmdir",
    "_wchdir",
    "_chdir",
    "_wchmod",
    "_chmod",
    "_wutime32",
    "_wutime64",
    "_utime32",
    "_utime64",
];
