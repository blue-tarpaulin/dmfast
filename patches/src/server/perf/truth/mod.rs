//! `truth`: `if(x)`, `!x`, `&&`, and `||` test numbers with an integer test instead of x87 (Linux).
//!
//! libbyond's true and false tests load a number onto the x87 stack and compare it with zero.
//! A number is false exactly when it is +0 or -0 (NaN is true), so `bits & 0x7FFFFFFF` gives the same answer.
//! Each test's number branch is rewritten in place and still answers in `al`, the only part of `eax` callers read.
//! The x87 status word no longer flags a signalling NaN but nothing reads it. Windows already compares with SSE.
//!
//! No dependency guard: each signature is the whole function.

use crate::base::feature::{Feature, Hosts, install};

#[cfg(target_os = "linux")]
mod linux;

pub static TRUTH: Feature = Feature::perf(Hosts::SERVER, "truth", "number truth test", install!(linux: linux::install));
