//! Running elevated (UAC relaunch) and keeping a window open long enough to read the result.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Console::GetConsoleProcessList;
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

/// Re-run this exe as admin with the same arguments (+ `--elevated`) and wait for it.
pub fn relaunch_elevated() -> bool {
    let exe = std::env::current_exe().unwrap();
    let mut params: Vec<String> = std::env::args().skip(1).map(|a| format!("\"{a}\"")).collect();
    params.push("--elevated".into());
    let (exe_w, params_w, verb) = (wide(exe.as_os_str()), wide(OsStr::new(&params.join(" "))), wide(OsStr::new("runas")));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = exe_w.as_ptr();
        info.lpParameters = params_w.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            return false;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        CloseHandle(info.hProcess);
    }
    true
}

/// True when this process has a console window to itself (double-click, drag-drop, elevated relaunch),
/// which would close before the output can be read. Not when run from a terminal.
fn own_console() -> bool {
    let mut pids = [0u32; 2];
    unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) == 1 }
}

pub fn pause_if_own_console() {
    if own_console() {
        println!("\npress Enter to close");
        let _ = io::stdin().read(&mut [0u8]);
    }
}
