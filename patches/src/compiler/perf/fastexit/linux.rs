//! DreamMaker fast exit.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// Return address of main's call to the compiler destructor.
static MAIN_RET: AtomicUsize = AtomicUsize::new(0);
/// `DungBuilder::~DungBuilder`, where the compiler destructor ends.
static BUILDER_DTOR: AtomicUsize = AtomicUsize::new(0);

/// Runs instead of `DungBuilder::~DungBuilder` when main destroys the compiler after the summary.
/// The rest of main's teardown writes no files (the `.rsc` is only unlocked and closed): flush stdio, stats, exit.
extern "C" fn fast_exit(code: i32) -> ! {
    // SAFETY: flushing all streams and exiting; main has finished with them.
    unsafe {
        libc::fflush(std::ptr::null_mut());
        crate::base::driver::print_stats();
        libc::_exit(code)
    }
}

/// Replaces the compiler destructor's tail `jmp DungBuilder::~DungBuilder`. Its frame is gone:
/// `[esp]` = its caller's return address, `[esp+4]` = this, ebx = the caller's ebx (main's exit code).
/// # Safety
/// Only jumped to from the compiler destructor's tail.
#[unsafe(naked)]
unsafe extern "C" fn dtor_tail() {
    naked_asm!(
        "push eax",
        "mov eax, dword ptr [{main_ret}]",
        "cmp dword ptr [esp + 4], eax",
        "pop eax",
        "jne 2f",
        "and esp, -16",
        "sub esp, 12",
        "push ebx",
        "call {fast_exit}",
        "2:",
        "jmp dword ptr [{dtor}]",
        main_ret = sym MAIN_RET,
        fast_exit = sym fast_exit,
        dtor = sym BUILDER_DTOR,
    )
}

/// Fast exit. Main, after printing the summary: `sub esp,0Ch ; push edi ; call compiler_dtor ;
/// lea eax,[ebp-x] ; mov [esp],eax ; call ~DMString ; ...`, with the exit code in ebx until it returns
/// (checked: from there main destroys two more locals and returns `mov eax,ebx`).
/// The compiler destructor joins the progress thread (516.1656+), frees a few buffers and ends with
/// `mov [ebp+8],esi ; lea esp,[ebp-8] ; pop ebx ; pop esi ; pop ebp ; jmp DungBuilder::~DungBuilder`, or
/// before 516.1656 (no frame pointer) `mov [esp+20h],esi ; add esp,14h ; pop ebx ; pop esi ; jmp ...`;
/// that jmp goes to [`dtor_tail`].
pub fn install(cx: &Ctx) -> Result<String> {
    let exe = cx.exe()?;
    let m = exe.find_unique(
        "83 EC 0C 57 E8 ?? ?? ?? ?? 8D 85 ?? FF FF FF 89 04 24 E8 ?? ?? ?? ?? 8D 85 ?? FF FF FF 89 04 24 E8",
        "main's compiler destructor call",
    )?;
    // The rest of main: add esp,10h ; (jmp epilogue) ; sub esp,0Ch ; push [ebp-x] ; call ; pop eax ;
    // push [ebp-y] ; call ; add esp,10h ; lea esp,[ebp-10h] ; mov eax,ebx: the exit code is ebx.
    let mut rest = m + 0x25;
    exe.expect(rest, &[0x83, 0xC4, 0x10], "main: unexpected code after the compiler destructor")?;
    rest += 3;
    match exe.bytes(rest, 1)?[0] {
        0xE9 => rest = exe.rel32(rest + 1)?,
        0xEB => rest = exe.rel8(rest + 1)?,
        _ => {}
    }
    let epilogue =
        "83 EC 0C FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 58 FF B5 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 8D 65 F0 89 D8";
    if !exe.is_at(rest, epilogue)? {
        bail!("main does not return the exit code from ebx");
    }
    let dtor = exe.rel32(m + 5)?;
    // The destructor's tail, ending in the jmp. A common tail: matches outside the destructor are other functions'
    let mut jmps = Vec::new();
    for tail in ["89 75 08 8D 65 F8 5B 5E 5D E9", "89 74 24 20 83 C4 14 5B 5E E9"] {
        let len = tail.split_whitespace().count();
        for t in exe.probe_all(tail)?.into_iter().filter(|&t| t > dtor && t < dtor + 0x100) {
            exe.rely(t, len)?;
            jmps.push(t + len - 1);
        }
    }
    // The deleting destructor's copy (same tail) can follow within the window: take the first.
    let jmp = jmps.into_iter().min().ok_or("compiler destructor tail not found")?;
    MAIN_RET.store(m + 9, Relaxed);
    BUILDER_DTOR.store(exe.rel32(jmp + 1)?, Relaxed);
    exe.jump(jmp, 5, dtor_tail as unsafe extern "C" fn())?;
    Ok("after summary".into())
}
