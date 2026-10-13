//! Stand-in for BYOND's `bin\WebView2Loader.dll`, which `byondwin.dll` imports first, making it the earliest
//! non-system DLL DreamSeeker initialises. It forwards the loader's four exports to the stock
//! `<byond>\dmfast\WebView2Loader.dll` and, when the host program is DreamSeeker, loads
//! `<byond>\dmfast\dmfast.dll`.
//!
//! Everything lives outside `bin\` on purpose: DreamSeeker enumerates its modules at startup and any DLL
//! under `bin\` that it does not know makes it malfunction later (which is why dreamseeker.exe's import
//! table cannot simply be rewritten like dd's). The copies of dmfast.dll in `bin\` serve dm/dd and are never
//! loaded here.
//!
//! No C runtime: the DLL imports kernel32 only and does nothing in programs other than DreamSeeker beyond
//! resolving the forwarded exports.
#![no_std]
#![allow(non_snake_case)]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of_mut, null_mut};

use windows_sys::Win32::Foundation::{FALSE, HMODULE, MAX_PATH, TRUE};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Marker the installer looks for (an export) to tell this DLL from Microsoft's.
#[unsafe(no_mangle)]
pub static dmfast_seekerproxy: u8 = 1;

/// The stock loader's exports, in ordinal order.
const NAMES: [&[u8]; 4] = [
    b"CompareBrowserVersions\0",
    b"CreateCoreWebView2Environment\0",
    b"CreateCoreWebView2EnvironmentWithOptions\0",
    b"GetAvailableCoreWebView2BrowserVersionString\0",
];

/// Filled in at attach; the export thunks jump through it.
static mut TARGETS: [*const c_void; 4] = [null_mut::<c_void>() as *const c_void; 4];

// One `jmp [target]` thunk per export, exported under the stock names and ordinals (the module has no
// other exports of its own besides the marker).
global_asm!(
    ".text",
    ".globl _dmfast_thunk0", "_dmfast_thunk0: jmp dword ptr [{t}]",
    ".globl _dmfast_thunk1", "_dmfast_thunk1: jmp dword ptr [{t}+4]",
    ".globl _dmfast_thunk2", "_dmfast_thunk2: jmp dword ptr [{t}+8]",
    ".globl _dmfast_thunk3", "_dmfast_thunk3: jmp dword ptr [{t}+12]",
    ".section .drectve,\"yn\"",
    ".ascii \" /EXPORT:CompareBrowserVersions=_dmfast_thunk0,@1\"",
    ".ascii \" /EXPORT:CreateCoreWebView2Environment=_dmfast_thunk1,@2\"",
    ".ascii \" /EXPORT:CreateCoreWebView2EnvironmentWithOptions=_dmfast_thunk2,@3\"",
    ".ascii \" /EXPORT:GetAvailableCoreWebView2BrowserVersionString=_dmfast_thunk3,@4\"",
    t = sym TARGETS,
);

/// A NUL-terminated UTF-16 path under construction. Lives in .bss (see [`DIR`]): a stack buffer would be
/// zero-initialised by a `memset` call, which needs the C runtime this DLL does without.
struct Path {
    buf: [u16; BUF],
    len: usize,
}

const BUF: usize = MAX_PATH as usize + 32;

impl Path {
    const fn new() -> Self {
        Path { buf: [0; BUF], len: 0 }
    }
    fn ptr(&mut self) -> *mut u16 {
        self.buf.as_mut_ptr()
    }
    fn get(&self, i: usize) -> u16 {
        self.buf[i]
    }
    fn set(&mut self, i: usize, v: u16) {
        self.buf[i] = v;
    }
    /// Fill from the module's file name.
    fn load(&mut self, module: HMODULE) {
        // SAFETY: writes at most MAX_PATH units, NUL-terminated by the API (or truncated at MAX_PATH).
        let n = unsafe { GetModuleFileNameW(module, self.ptr(), MAX_PATH) } as usize;
        self.len = n.min(MAX_PATH as usize - 1);
        self.set(self.len, 0);
    }
    fn push(&mut self, s: &[u8]) {
        let mut i = 0;
        while i < s.len() && self.len + 1 < BUF {
            self.set(self.len, s[i] as u16);
            self.len += 1;
            i += 1;
        }
        self.set(self.len, 0);
    }
    /// Cut the path down to just before its `n`th last `\` (1 = the file name, 2 = the folder too).
    fn pop_components(&mut self, mut n: usize) {
        while n > 0 {
            while self.len > 0 && self.get(self.len - 1) != b'\\' as u16 {
                self.len -= 1;
            }
            self.len = self.len.saturating_sub(1);
            n -= 1;
        }
        self.set(self.len, 0);
    }
}

static mut DIR: Path = Path::new();
static mut HOST: Path = Path::new();

/// Whether the running program's file name is `dreamseeker.exe` (any case).
fn host_is_dreamseeker(p: &mut Path) -> bool {
    p.load(null_mut());
    let want = b"dreamseeker.exe";
    if p.len < want.len() {
        return false;
    }
    let start = p.len - want.len();
    if start > 0 && p.get(start - 1) != b'\\' as u16 {
        return false;
    }
    let mut i = 0;
    while i < want.len() {
        if p.get(start + i) | 0x20 != want[i] as u16 {
            return false;
        }
        i += 1;
    }
    true
}

// The entry point is a jump to `dll_main`, so that no exported Rust symbol is needed for it.
global_asm!(".text", ".globl _dmfast_entry", "_dmfast_entry: jmp {main}", main = sym dll_main);

extern "system" fn dll_main(module: HMODULE, reason: u32, _: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return TRUE;
    }
    // <byond>\bin\WebView2Loader.dll -> <byond>\dmfast\
    // SAFETY: DllMain runs under the loader lock, once per process attach; nothing else touches these.
    let (dir, host) = unsafe { (&mut *addr_of_mut!(DIR), &mut *addr_of_mut!(HOST)) };
    dir.load(module);
    dir.pop_components(2);
    dir.push(b"\\dmfast\\");
    let base = dir.len;

    dir.push(b"WebView2Loader.dll");
    // SAFETY: `dir` is NUL-terminated; the thunks are only reached after this wrote their targets. Without
    // the stock loader or one of its exports, attach fails: the load error a missing import gives too,
    // rather than a jump through null at the first WebView call.
    unsafe {
        let loader = LoadLibraryExW(dir.ptr(), null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
        if loader.is_null() {
            return FALSE;
        }
        let targets = addr_of_mut!(TARGETS).cast::<*const c_void>();
        let mut i = 0;
        while i < NAMES.len() {
            let Some(f) = GetProcAddress(loader, NAMES[i].as_ptr()) else { return FALSE };
            targets.add(i).write(f as *const c_void);
            i += 1;
        }
    }

    if host_is_dreamseeker(host) {
        // dmfast_early.dll first, as in the patched programs: it must run before mimalloc (dmfast.dll's
        // import, resolved from the same folder) starts.
        for name in [&b"dmfast_early.dll"[..], b"dmfast.dll"] {
            dir.len = base;
            dir.push(name);
            // SAFETY: `dir` is NUL-terminated.
            unsafe { LoadLibraryExW(dir.ptr(), null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        }
    }
    TRUE
}
