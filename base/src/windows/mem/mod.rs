//! Finding and patching code in a loaded module: the image (`image`, [`crate::image::Image`]'s Windows part),
//! writes to it and executable memory for generated code (`patch`).
//!
//! Patches run after the loader relocated the module, so absolute addresses in code are already live.

mod image;
mod patch;

pub use image::module_range;
// Windows writes data like code, through the page protection.
pub(crate) use patch::{alloc_exec, free_exec, poke_code, poke_code as poke_data, seal_exec};
