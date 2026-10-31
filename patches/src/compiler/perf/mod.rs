//! Compiler speed patches: the `.dmb` stays identical to stock.

use crate::base::feature::Ctx;

pub mod banner;
pub mod ctype;
pub mod dirs;
pub mod dmbbuf;
pub mod fastexit;
pub mod fscache;
pub mod listinit;
pub mod poll;
pub mod vecgrow;

/// The compiler executable is known (Linux: the loader can leave the main program out of its list).
fn found(c: &Ctx) -> bool {
    c.exe().is_ok()
}
