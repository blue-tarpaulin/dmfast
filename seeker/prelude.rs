//! What every DreamSeeker patch compiles with (`author::compile` makes it the crate's `prelude` module, before the
//! patch): the built-in addresses a body jumps to or uses, and `text!` for its strings. Use it with
//! `use crate::prelude::*;`.

// A patch uses only some of it.
#![allow(dead_code, unused_macros)]

unsafe extern "C" {
    /// The instruction after the site.
    pub fn dmf_resume();
    /// The site's original instructions, then the instruction after the site.
    pub fn dmf_original();
    /// The patch's zero-filled data (`data` in its spec).
    pub static dmf_data: u8;
}

/// `text!(NAME = "...")`: a NUL-terminated string constant named `NAME` (`sym NAME` in a body). The text may be any
/// constant `&str`, such as `concat!("... dmfast ", env!("DMFAST_VERSION"))`.
macro_rules! text {
    ($name:ident = $text:expr) => {
        #[unsafe(no_mangle)]
        pub static $name: [u8; $text.len() + 1] = $crate::prelude::with_nul($text);
    };
}

/// `text` and a NUL, as `N` (its length plus one) bytes.
pub const fn with_nul<const N: usize>(text: &str) -> [u8; N] {
    let (mut out, text) = ([0; N], text.as_bytes());
    let mut i = 0;
    while i < text.len() {
        out[i] = text[i];
        i += 1;
    }
    out
}
