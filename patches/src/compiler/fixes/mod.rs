//! Compiler bug fixes: opt-in, listed in BUGFIXES.md, each with a test world in `tests/fixes/<key>.dme`.

use std::sync::OnceLock;

use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

pub mod contline;
pub mod finalpath;
pub mod indent;
pub mod macrospace;
pub mod mapcolors;
pub mod objworld;
pub mod regexstart;
pub mod typepath;
pub mod varlimit;

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

/// The compiler's name table (names by id), from its interning loop.
#[cfg(windows)]
fn name_table(core: &Image) -> Result<usize> {
    // mov esi,[edx] ; mov ebx,edx ; mov ecx,[eax+names+4] ; mov [ebp-10h],ecx ; test esi,esi
    Ok(core.ptr_at(core.find_unique("8B 32 8B DA 8B 88 ?? ?? ?? ?? 89 4D F0 85 F6", "name table")? + 6)? - 4)
}

/// The compiler's name table (names by id), from its interning loop: push [ebx*4+names+4] ; add ebx,1 ; push esi ;
/// call intern ; add esp,10h ; cmp ebx,count ; jne ; cmp [esi+88h],count (two inlined copies, one table).
#[cfg(target_os = "linux")]
fn name_table(core: &Image) -> Result<usize> {
    let hits = core.find_all(
        "FF 34 9D ?? ?? ?? ?? 83 C3 01 56 E8 ?? ?? ?? ?? 83 C4 10 81 FB ?? ?? ?? ?? 75 E2 81 BE 88 00 00 00",
    )?;
    let table = core.ptr_at(hits.first_or("name table")? + 3)?;
    for &h in hits.iter() {
        ensure!(core.ptr_at(h + 3)? == table, "name table: the interning loops read different tables");
    }
    Ok(table - 4)
}

/// The name of `id` in the name table at `names`, if it has one.
fn name_of(core: &Image, names: usize, id: usize) -> Result<Option<&[u8]>> {
    let p = core.ptr_at(names + id * 4)?;
    if p == 0 { Ok(None) } else { core.c_str(p).map(Some) }
}

/// Checks that each `(id, name)` is in the compiler's name table.
fn require_names(core: &Image, ids: &[(u32, &str)]) -> Result<()> {
    let names = name_table(core)?;
    for &(id, name) in ids {
        ensure!(name_of(core, names, id as usize)? == Some(name.as_bytes()), "name table id {id:#x} is not {name}");
    }
    Ok(())
}
