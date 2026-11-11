//! Test-only patch code with a static that isn't zero-initialised, which a patch can't have.

static mut SEED: u32 = 5;

#[unsafe(no_mangle)]
pub extern "C" fn nonzero() -> u32 {
    let seed = &raw mut SEED;
    unsafe {
        *seed = (*seed).wrapping_mul(3);
        *seed
    }
}
