// The call_ext() functions callextleak.dme calls, which do nothing. Built by callextleak-gen.sh.
use std::ffi::c_char;

/// byondapi's CByondValue.
#[repr(C)]
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
