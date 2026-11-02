//! `macrospace` (forum reports 2988673 and 2976632): the space inserted after a macro's expansion no longer breaks
//! `::`, `?.`, and `.` right after the macro (`COLOR::blue`, `macro(x)?.var`, `macro(x).var`).
//!
//! An expansion is pushed as a text stream ending in spaces (two for a function-like macro), which keep `+` `PLUS`
//! from pasting into `++`. The text stream's reader is replaced by a copy that asks [`drop_space`] before returning a
//! final space: it's dropped when the enclosing stream's next character is `:`, `?`, or `.` and the space follows an
//! identifier, closing bracket, or string (not a number before `.`). Before `:` after an identifier it's dropped only
//! for `::`, since `c ? MACRO:b` needs it.

use std::ffi::{CStr, c_void};

use crate::base::code::{self, Foreign, live_u32};
use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::Installed;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static MACROSPACE: Feature = Feature::fix(
    Hosts::COMPILER,
    "macrospace",
    "no space after a macro before : ? .",
    Tier::Safe,
    install!(windows: windows::install, linux: linux::install),
);

/// A text stream's text (NUL-terminated), position in it, and current character (the reader's result).
const TEXT: usize = 0x34;
const POS: usize = 0x3C;
const CURRENT: usize = 0x40;
/// What a stream's FILE is at, for a file stream.
const FILE: usize = 8;

type Getc = unsafe extern "C" fn(*mut c_void) -> i32;
type Ungetc = unsafe extern "C" fn(i32, *mut c_void) -> i32;

/// What install found: the text stream's layout, the readers [`peek`] tells apart, and the C library's `getc` and
/// `ungetc` for a file stream's FILE.
struct Found {
    /// The enclosing stream.
    parent: usize,
    /// The kind byte, nonzero for macro text.
    kind: usize,
    /// Where a stream keeps the lexer's current character while a stream pushed on top of it is read.
    saved: usize,
    /// The reader's vtable slot, and the text and file streams' readers.
    slot: usize,
    text_reader: usize,
    file_reader: usize,
    getc: Foreign<Getc>,
    ungetc: Foreign<Ungetc>,
}

static FOUND: Installed<Found> = Installed::new();

/// The enclosing stream's next character after the one the lexer holds, not consumed; -1 if unknown.
///
/// # Safety
/// `parent` is a live stream.
unsafe fn peek(f: &Found, parent: usize) -> i32 {
    // SAFETY: a live stream (caller): its vtable, and its text or FILE.
    unsafe {
        let reader = live_u32(live_u32(parent) as usize + 4 * f.slot) as usize;
        if reader == f.text_reader {
            let text = live_u32(parent + TEXT) as usize;
            return if text == 0 { -1 } else { i32::from(code::read::<u8>(text + live_u32(parent + POS) as usize)) };
        }
        if reader != f.file_reader {
            return -1;
        }
        let file = live_u32(parent + FILE) as usize as *mut c_void;
        let c = f.getc.get()(file);
        if c != -1 {
            f.ungetc.get()(c, file);
        }
        c
    }
}

/// Whether text stream `stream` is at its final space and should drop it.
///
/// # Safety
/// `stream` is a live text stream (the reader passes it).
unsafe extern "C" fn drop_space(stream: usize) -> u32 {
    let f = FOUND.get();
    // SAFETY: a live text stream (caller), its parent, and its NUL-terminated text.
    unsafe {
        if code::read::<u8>(stream + f.kind) == 0 {
            return 0;
        }
        let parent = live_u32(stream + f.parent) as usize;
        if parent == 0 {
            return 0;
        }
        let next = live_u32(parent + f.saved) as i32;
        if !matches!(u8::try_from(next), Ok(b':' | b'?' | b'.')) {
            return 0;
        }
        let (text, pos) = (live_u32(stream + TEXT) as usize, live_u32(stream + POS) as usize);
        if text == 0 {
            return 0;
        }
        // Only spaces may remain: a function-like macro's text ends in two (its argument's own and the final one).
        let rest = CStr::from_ptr(text as *const _).to_bytes();
        if pos == 0 || pos >= rest.len() || rest[pos..].iter().any(|&c| c != b' ') {
            return 0;
        }
        let Some(end) = rest[..pos].iter().rposition(|&c| c != b' ').map(|i| i + 1) else { return 0 };
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let prev = rest[end - 1];
        if !(word(prev) || matches!(prev, b')' | b']' | b'"')) {
            return 0;
        }
        if next == i32::from(b'.') && word(prev) {
            // A number before `.` would read as a different number (`1` `.5`): drop only after an identifier.
            let head = &rest[..end];
            let start = head.iter().rposition(|&c| !word(c)).map_or(0, |i| i + 1);
            if head[start].is_ascii_digit() {
                return 0;
            }
        }
        if next == i32::from(b':') && word(prev) && peek(f, parent) != i32::from(b':') {
            // `X:y` right after a name is `X`'s member, `X :y` a ternary's else: keep the space unless it's `::`.
            return 0;
        }
        1
    }
}
