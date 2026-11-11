//! Test-only patch body that indexes a slice: its bounds check calls core's panic path, which `author` points at the
//! prelude's trap, and passes a location (an address in the constants, to the file name).

static mut BUF: [u8; 16] = [0; 16];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn index(i: usize) -> u8 {
    unsafe { (*(&raw const BUF))[i] }
}
