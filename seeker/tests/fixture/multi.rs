//! Test-only patch code in plain Rust: a call between two of its functions, a function's address taken, two
//! zero-initialised statics, and a Direct3D call through `com3`.

use crate::prelude::*;

// Exported, so LLVM keeps them (and the store to `HOOK`, which nothing in the object reads).
#[unsafe(no_mangle)]
pub static mut COUNT: u32 = 0;
#[unsafe(no_mangle)]
pub static mut HOOK: usize = 0;

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn bump(by: u32) -> u32 {
    let count = &raw mut COUNT;
    unsafe {
        *count = (*count).wrapping_add(by);
        *count
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn multi(device: P, a: usize) -> i32 {
    unsafe {
        *(&raw mut HOOK) = bump as *const () as usize;
        let n = bump(a as u32);
        com3(device, 17, n as usize, 2, 3)
    }
}
