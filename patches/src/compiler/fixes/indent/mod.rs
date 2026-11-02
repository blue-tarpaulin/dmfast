//! `indent` (forum report 2970135): a line indented with mixed tabs and spaces is an `inconsistent indentation` error
//! instead of silently opening an unintended block.
//!
//! The lexer counts every leading tab or space as one unit. The fix records each character its counting loop sees in
//! [`BUF`] and, where a line is accepted as code, raises BYOND's own `inconsistent indentation` error when [`check`]
//! rejects the line's prefix. Blank and comment-only lines pass.
//!
//! Python's rule: a line's leading whitespace must equal the current block's prefix, extend it (a deeper block), or
//! equal an enclosing block's prefix (a dedent). A consistently mixed prefix passes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use crate::base::feature::{Feature, Hosts, Tier, install};
use crate::base::sync::lock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub static INDENT: Feature = Feature::fix(
    Hosts::COMPILER,
    "indent",
    "mixed tab/space indentation is rejected",
    Tier::Breaking,
    install!(windows: windows::install, linux: linux::install),
);

/// The longest leading whitespace recorded; a longer line isn't checked.
const CAP: usize = 64;

/// The leading whitespace of the line being counted (the stubs write `BUF[count]`).
static BUF: [AtomicU8; CAP] = [const { AtomicU8::new(0) }; CAP];

/// The prefixes of the open blocks, outermost first; the implicit base is the empty prefix.
static STACK: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Whether `prefix` is consistent with the open blocks in `stack`, which it then updates.
fn consistent(stack: &mut Vec<Vec<u8>>, prefix: &[u8]) -> bool {
    while stack.last().is_some_and(|top| top.len() > prefix.len()) {
        stack.pop();
    }
    // After an inconsistent line its prefix becomes the block's prefix, so one mistake is reported once.
    let ok = match stack.last() {
        Some(top) if top.len() == prefix.len() => {
            let ok = top == prefix;
            if !ok {
                stack.pop();
            }
            ok
        }
        Some(top) => top.is_empty() || prefix.starts_with(top),
        None => true,
    };
    if !prefix.is_empty() && stack.last().is_none_or(|top| top.len() < prefix.len()) {
        stack.push(prefix.to_vec());
    }
    ok
}

/// Whether the line accepted as code, with `count` leading whitespace characters, is inconsistent (nonzero).
extern "C" fn check(count: u32) -> u32 {
    let count = count as usize;
    if count >= CAP {
        return 0;
    }
    let prefix: Vec<u8> = BUF[..count].iter().map(|b| b.load(Relaxed)).collect();
    u32::from(!consistent(&mut lock(&STACK), &prefix))
}
