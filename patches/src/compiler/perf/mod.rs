//! Compiler speed patches: the `.dmb` stays identical to stock.

use crate::base::feature::Ctx;

pub mod banner;
pub mod children;
pub mod ctype;
pub mod dirs;
pub mod dmbbuf;
pub mod fastexit;
pub mod filter;
pub mod fscache;
pub mod intern;
pub mod leafskip;
pub mod listinit;
pub mod lists;
pub mod parent;
pub mod poll;
pub mod protos;
pub mod resources;
pub mod slots;
pub mod stdio;
pub mod strings;
pub mod strtab;
pub mod vecgrow;

/// The compiler executable is known (Linux: the loader can leave the main program out of its list).
fn found(c: &Ctx) -> bool {
    c.exe().is_ok()
}
