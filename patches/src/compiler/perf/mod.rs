//! Compiler speed patches: the `.dmb` stays identical to stock.

use crate::base::feature::Ctx;

pub mod banner;
pub mod fastexit;
pub mod listinit;
pub mod poll;

/// The compiler executable is known (Linux: the loader can leave the main program out of its list).
fn found(c: &Ctx) -> bool {
    c.exe().is_ok()
}
