//! Minimal PE32 reading/editing: section names, exports (the canary), and prepending an import descriptor (the
//! section append is the seeker crate's).

use dmfast_seeker::pe::{self as image, DATA_FLAGS, DIR_IMPORT, NewSection, Pe, put32};
use object::read::pe::{ExportTarget, PeFile32};

type Result<T, E = String> = std::result::Result<T, E>;

const CANARY_EXPORT: &[u8] = b"dmfast_canary";
/// Section added to dm.exe for the new import table.
const IMPORT_SECTION: [u8; 8] = *b".dmfimp\0";

fn parse(data: &[u8]) -> Result<PeFile32<'_>> {
    PeFile32::parse(data).map_err(|e| format!("not a 32-bit PE file: {e}"))
}

pub fn has_section(data: &[u8], prefix: &[u8]) -> bool {
    parse(data).is_ok_and(|pe| pe.section_table().iter().any(|s| s.name.starts_with(prefix)))
}

/// Whether the image imports `dll` (case-insensitive).
pub fn imports(data: &[u8], dll: &str) -> Result<bool> {
    let pe = parse(data)?;
    let table = pe.import_table().map_err(|e| e.to_string())?.ok_or("no import table")?;
    let mut descs = table.descriptors().map_err(|e| e.to_string())?;
    while let Some(d) = descs.next().map_err(|e| e.to_string())? {
        let name = table.name(d.name.get(object::LittleEndian)).map_err(|e| e.to_string())?;
        if name.eq_ignore_ascii_case(dll.as_bytes()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The dmfast canary (`DMFAST <version>`) exported by a dmfast.dll.
pub fn canary(data: &[u8]) -> Option<String> {
    let pe = parse(data).ok()?;
    let table = pe.export_table().ok()??;
    let e = table.exports().ok()?.into_iter().find(|e| e.name == Some(CANARY_EXPORT))?;
    let ExportTarget::Address(rva) = e.target else { return None };
    let bytes = pe.section_table().pe_data_at(data, rva)?;
    let end = bytes.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// Copy of `exe` whose import table starts with `dll!func` for each of `first`, in order, so the loader
/// loads and initialises those DLLs (and their own imports) before the exe's other imports. The original
/// descriptors are kept as-is; the new table lives in an appended section.
pub fn add_imports_first(exe: &[u8], first: &[(&str, &str)]) -> Result<Vec<u8>> {
    let pe = Pe::parse(exe)?;
    // Existing descriptors (without the terminator).
    let (idir_rva, idir_size) = pe.dir(DIR_IMPORT);
    let old = pe.bytes(idir_rva, idir_size).ok_or("import directory not in a section")?;
    let old: Vec<&[u8; 20]> = old.as_chunks::<20>().0.iter().take_while(|c| c.iter().any(|&b| b != 0)).collect();

    let va = pe.next_rva();
    let n = first.len() + old.len() + 1; // ours + old + terminator
    let mut blob = vec![0u8; 20 * n];
    let put = |blob: &mut Vec<u8>, data: &[u8], alignment: usize| {
        while !blob.len().is_multiple_of(alignment) {
            blob.push(0);
        }
        let rva = va + blob.len() as u32;
        blob.extend_from_slice(data);
        rva
    };
    for (k, (dll, func)) in first.iter().enumerate() {
        let name = put(&mut blob, &[dll.as_bytes(), b"\0"].concat(), 2);
        let hint_name = put(&mut blob, &[&[0u8, 0][..], func.as_bytes(), b"\0"].concat(), 2);
        let thunks = [hint_name.to_le_bytes(), [0; 4]].concat();
        let ilt = put(&mut blob, &thunks, 4);
        let iat = put(&mut blob, &thunks, 4);
        for (i, v) in [ilt, 0, 0, name, iat].into_iter().enumerate() {
            put32(&mut blob, 20 * k + 4 * i, v);
        }
    }
    for (i, desc) in old.iter().enumerate() {
        blob[20 * (first.len() + i)..20 * (first.len() + i + 1)].copy_from_slice(*desc);
    }

    // Initialised data, read and write: the loader writes the IAT.
    let section = NewSection { name: IMPORT_SECTION, flags: DATA_FLAGS, data: &blob, vsize: 0 };
    let d = image::append(&pe, &[section], &[(DIR_IMPORT, va, 20 * n as u32)])?;

    for (dll, _) in first {
        if !imports(&d, dll)? {
            return Err("internal: import table rewrite failed".into());
        }
    }
    Ok(d)
}
