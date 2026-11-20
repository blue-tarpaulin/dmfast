// The call_ext() functions callextleak.dme and awaitstack.dme call. Built by their -gen.sh.
use std::ffi::{c_char, c_void};

/// byondapi's CByondValue.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Value {
    kind: u8,
    pad: [u8; 3],
    data: u32,
}

/// A byond: function.
#[no_mangle]
pub extern "C" fn noop(_n: u32, _args: *const Value) -> Value {
    Value { kind: 0, pad: [0; 3], data: 0 }
}

/// A text function (call_ext()'s older kind).
#[no_mangle]
pub extern "C" fn noop_text(_argc: i32, _argv: *const *const c_char) -> *const c_char {
    c"".as_ptr()
}

type ReturnFn = unsafe extern "C" fn(*const Value, *const Value) -> bool;

/// byondapi's Byond_Return.
#[cfg(windows)]
fn byond_return() -> ReturnFn {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    unsafe {
        std::mem::transmute(GetProcAddress(GetModuleHandleA(c"byondcore.dll".as_ptr()), c"Byond_Return".as_ptr()))
    }
}
#[cfg(not(windows))]
fn byond_return() -> ReturnFn {
    #[link(name = "dl")]
    extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    unsafe { std::mem::transmute(dlsym(std::ptr::null_mut(), c"Byond_Return".as_ptr())) }
}

/// 1234, an await function's result.
const RESULT: Value = Value { kind: 0x2A, pad: [0; 3], data: 0x449A_4000 };

/// A byond,await: function that returns 1234 from another thread.
#[no_mangle]
pub extern "C" fn await_later(_n: u32, _args: *const Value, waiter: Value) {
    let ret = byond_return();
    std::thread::spawn(move || unsafe {
        ret(&waiter, &RESULT);
    });
}

/// A byond,await: function that returns 1234 before it returns.
#[no_mangle]
pub extern "C" fn await_now(_n: u32, _args: *const Value, waiter: Value) {
    unsafe { byond_return()(&waiter, &RESULT) };
}
