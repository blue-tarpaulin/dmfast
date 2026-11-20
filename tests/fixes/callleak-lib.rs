// The call_ext() functions callleak.dme calls. Built by callleak-gen.sh.
use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Value {
    kind: u8,
    pad: [u8; 3],
    data: u32,
}

#[no_mangle]
pub extern "C" fn noop(_n: u32, _args: *const Value) -> Value {
    Value { kind: 0, pad: [0; 3], data: 0 }
}

#[no_mangle]
pub extern "C" fn noop_text(_argc: i32, _argv: *const *const c_char) -> *const c_char {
    c"".as_ptr()
}

type ReturnFn = unsafe extern "C" fn(*const Value, *const Value) -> bool;

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

const RESULT: Value = Value { kind: 0x2A, pad: [0; 3], data: 0x449A_4000 };

#[no_mangle]
pub extern "C" fn await_later(_n: u32, _args: *const Value, waiter: Value) {
    let ret = byond_return();
    std::thread::spawn(move || unsafe {
        ret(&waiter, &RESULT);
    });
}

#[no_mangle]
pub extern "C" fn await_now(_n: u32, _args: *const Value, waiter: Value) {
    unsafe { byond_return()(&waiter, &RESULT) };
}
