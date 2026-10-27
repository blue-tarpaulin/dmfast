//! Finding (`image`: [`crate::image::Image`]'s Linux part: segments, relocations, exports, import calls) and
//! patching (`patch`) code and data in a loaded object, and executable memory for generated code. Patches run
//! after relocation, so absolute addresses in code are the live ones (libbyond.so has text relocations).

mod image;
mod patch;

pub(crate) use patch::{alloc_exec, free_exec, poke_code, poke_data, seal_exec};
