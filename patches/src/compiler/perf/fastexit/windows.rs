//! dm.exe fast exit.

use std::sync::OnceLock;

use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE, WriteConsoleW};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

use crate::base::code::{Foreign, live_u32};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::windows::ucrt::ucrt_export;
use crate::base::{Result, bail};

/// What [`fast_exit`] redoes of the skipped teardown; set when the feature commits.
static EXIT: OnceLock<Exit> = OnceLock::new();

struct Exit {
    /// ucrtbase's `_flushall`.
    flushall: Foreign<unsafe extern "C" fn() -> i32>,
    /// byondcore's list of open resource files with lock files (+0 lock path, +18h next), its entries' data FILE*
    /// field (+8 in 515-516) and its release function (cdecl, data FILE*).
    locks: usize,
    data_file: usize,
    release: Foreign<extern "C" fn(usize) -> u32>,
    status: Option<Status>,
}

/// Replaces the builder teardown once the .dmb is written and the summary printed: clear the status line, release
/// the resource locks, flush every CRT stream, print stats and exit with main's code.
extern "C" fn fast_exit(code: i32) -> ! {
    let Some(exit) = EXIT.get() else { std::process::abort() };
    // SAFETY: install found and checked these addresses; main has finished with them.
    unsafe {
        if let Some(status) = &exit.status {
            status.clear();
        }
        // The release per open resource file: fclose its lock file, delete the `.lk`, flush the data file.
        for _ in 0..1024 {
            let head = live_u32(exit.locks) as usize;
            if head == 0 {
                break;
            }
            exit.release.get()(live_u32(head + exit.data_file) as usize);
        }
        exit.flushall.get()();
        crate::base::driver::print_stats();
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::abort()
}

/// At the end of main, after "Total time" is printed:
/// `lea ecx,[ebp-x] ; call builder_teardown ; lea ecx,[ebp-y] ; call ebx ; lea ecx,[ebp-z] ; call ebx`, then the
/// epilogue (inline, or behind a `jmp` in newer builds): two destructors, then `mov eax,[ebp-r]` (the exit code).
/// The first two instructions become `push dword [ebp-r] ; call fast_exit`.
pub fn install(cx: &Ctx) -> Result<String> {
    let (exe, core) = (cx.exe()?, cx.core());
    // `_flushall` flushes ucrtbase's streams: both modules must use them.
    exe.ucrt_slot("fflush")?;
    core.ucrt_slot("fclose")?;
    let flushall = ucrt_export("_flushall")?;
    // Lock release: SEH prologue ; ... ; mov eax,[list] ; mov esi,eax ; test esi,esi ; jz ;
    // mov edi,[ebp+8] ; search: cmp [esi+file],edi ; je
    let release = core.find_unique(
        "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 51 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
         64 A3 00 00 00 00 A1 ?? ?? ?? ?? 8B F0 85 F6 0F 84 ?? ?? ?? ?? 8B 7D 08 39 7E ?? 74",
        "resource lock release",
    )?;
    let locks = core.ptr_at(release + 38)?;
    if core.find_bytes_in(release, 0x100, &(locks as u32).to_le_bytes())?.len() < 2 {
        bail!("resource lock list not where expected");
    }
    let data_file = usize::from(core.bytes(release + 57, 1)?[0]);
    let m = exe.find_unique("8D 8D ?? ?? FF FF E8 ?? ?? ?? ?? 8D 4D ?? FF D3 8D 4D ?? FF D3", "builder teardown")?;
    let epilogue = if exe.bytes(m + 21, 1)? == [0xE9] { exe.rel32(m + 22)? } else { m + 21 };
    // lea ecx,[ebp-a] ; call ; lea ecx,[ebp-b] ; call ; mov eax,[ebp-r] ; mov ecx,[ebp-0Ch]
    let tail = "8D 8D ?? ?? ?? ?? E8 ?? ?? ?? ?? 8D 8D ?? ?? ?? ?? E8 ?? ?? ?? ?? 8B 45 ?? 8B 4D F4";
    if !exe.is_at(epilogue, tail)? {
        bail!("unexpected epilogue after the builder teardown");
    }
    let ret = exe.bytes(epilogue + 24, 1)?[0];
    let status = Status::find(exe, exe.rel32(m + 7)?)?;
    exe.write_code(m, &[0xFF, 0x75, ret])?; // push dword [ebp-r]
    exe.call(m + 3, 5, fast_exit as *const ())?;
    let exit = Exit { flushall: Foreign::at(flushall), locks, data_file, release: Foreign::at(release), status };
    cx.on_commit(move || {
        let _ = EXIT.set(exit);
    });
    Ok("after summary".into())
}

/// The console status line ("Saving... |"): its object (a mutex first), the mutex's Lock (thiscall) and the offsets
/// of its "showing" and "console output" bytes.
struct Status {
    object: usize,
    lock: Foreign<extern "thiscall" fn(usize)>,
    showing: usize,
    console: usize,
}

impl Status {
    /// The status line the skipped `teardown` (the progress display's destructor) clears, found as
    /// `mov ecx,<object> ; call printer` in it. `None` in builds without one (before 516.1656).
    fn find(exe: &Image, teardown: usize) -> Result<Option<Status>> {
        // SEH prologue ; mov esi,ecx ; xor ebx,ebx ; ... ; call [DungMutex::Lock] ; mov edi,[DMString dtor] ;
        // mov byte [ebp-4],1 ; cmp [esi+console],bl ; je
        let Ok(printer) = exe.find_unique(
            "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 14 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 \
             64 A3 00 00 00 00 8B F1 33 DB 89 5D EC 89 5D FC 89 75 E8 FF 15 ?? ?? ?? ?? 8B 3D ?? ?? ?? ?? C6 45 FC 01 \
             38 5E ?? 0F 84",
            "status printer",
        ) else {
            return Ok(None);
        };
        let lock_slot = exe.iat_slot("?Lock@DungMutex@@QAEXXZ")?;
        if exe.ptr_at(printer + 0x37)? != lock_slot {
            bail!("status printer: unexpected lock");
        }
        let console = usize::from(exe.bytes(printer + 0x47, 1)?[0]);
        // Past the console check, the printer draws only while `cmp byte [esi+showing],0` is nonzero (twice in 516).
        let mut flags = exe
            .find_in(printer + 0x4B, 0x200 - 0x4B, "80 7E ?? 00")?
            .into_iter()
            .map(|at| Ok(exe.bytes(at + 2, 1)?[0]))
            .collect::<Result<Vec<u8>>>()?;
        flags.dedup();
        let [showing] = flags[..] else { bail!("status printer: expected one \"showing\" flag, found {flags:?}") };
        let call = exe.calls_in(teardown + 5, 0xFF - 5, printer)?.first().ok_or("status printer call not found")?;
        exe.expect(call - 5, &[0xB9], "status line object not found in the teardown")?;
        Ok(Some(Status {
            object: exe.ptr_at(call - 4)?,
            lock: Foreign::at(exe.ptr_at(lock_slot)?),
            showing: usize::from(showing),
            console,
        }))
    }

    /// Clear the line if it is showing on the console. The lock is kept until exit so the progress thread can't
    /// redraw it.
    ///
    /// # Safety
    /// Found by [`Status::find`]; main has finished.
    unsafe fn clear(&self) {
        // SAFETY: the object and its Lock were found and checked at install.
        unsafe {
            self.lock.get()(self.object);
            let flag = |offset: usize| *((self.object + offset) as *const u8) != 0;
            if flag(self.showing) && flag(self.console) {
                let text = b"\x1b[2K\r".map(u16::from);
                let out = GetStdHandle(STD_OUTPUT_HANDLE);
                WriteConsoleW(out, text.as_ptr().cast(), text.len() as u32, std::ptr::null_mut(), std::ptr::null());
            }
        }
    }
}
