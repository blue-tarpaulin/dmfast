//! Compiler bug fixes: opt-in, listed in BUGFIXES.md, each with a test world in `tests/fixes/<key>.dme`.

use std::sync::OnceLock;

use crate::base::image::Image;
use crate::base::{Result, bail};

pub mod contline;
pub mod mapcolors;
pub mod regexstart;
pub mod typepath;

/// Replaces the built-in DM source chunk containing `mark` (one of a table of NUL-terminated chunks the compiler
/// parses first) with `edit` of its text, kept in `fixed` for the life of the process. Returns the table entry.
fn replace_source(
    core: &Image,
    mark: &[u8],
    what: &str,
    fixed: &'static OnceLock<Vec<u8>>,
    edit: impl FnOnce(&str) -> Result<String>,
) -> Result<usize> {
    let chunk = core.find_bytes(mark)?.unique(what)?;
    let Ok(text) = std::str::from_utf8(core.c_str(chunk)?) else { bail!("{what} is not UTF-8") };
    let edited = edit(text)?;
    let new = fixed.get_or_init(|| format!("{edited}\0").into_bytes());
    let slot = core.find_data(&(chunk as u32).to_le_bytes())?.unique(&format!("{what} table entry"))?;
    core.write_data(slot, &(new.as_ptr() as u32).to_le_bytes())?;
    Ok(slot)
}
