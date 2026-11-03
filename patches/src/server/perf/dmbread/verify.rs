//! `DMFAST_VERIFY=1`: each `ftell` answered without the CRT is checked against the CRT's; on a disagreement the CRT
//! answers and the stream's position is forgotten.

use crate::base::driver::{Counter, verify};

pub(super) static MISMATCHES: Counter = Counter::new();

/// With verify on, whether `real` (the CRT's `ftell`) differs from the answer `pos`, counted.
pub(super) fn disagrees<T: PartialEq>(pos: T, real: impl FnOnce() -> T) -> bool {
    let wrong = verify() && real() != pos;
    if wrong {
        MISMATCHES.bump();
    }
    wrong
}
