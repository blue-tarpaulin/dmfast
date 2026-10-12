//! Minimal PE32 reading/editing: section names, canary export, and prepending an import descriptor.

use object::read::pe::{ExportTarget, PeFile32};

type Result<T, E = String> = std::result::Result<T, E>;

const CANARY_EXPORT: &[u8] = b"dmfast_canary";
/// Section added to dm.exe for the new import table.
const IMPORT_SECTION: &[u8; 8] = b".dmfimp\0";

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

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn align(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

/// Copy of `exe` whose import table starts with `dll!func`, so the loader loads `dll` (and its own
/// imports, e.g. mimalloc) before the exe's other imports. The original descriptors are kept as-is; the
/// new table lives in an appended section.
pub fn add_import_first(exe: &[u8], dll: &str, func: &str) -> Result<Vec<u8>> {
    let mut d = exe.to_vec();
    let fh = u32_at(&d, 0x3C) as usize + 4;
    let nsec = u16_at(&d, fh + 2) as usize;
    let opt = fh + 20;
    if u16_at(&d, opt) != 0x10B {
        return Err("not a PE32 image".into());
    }
    let sec_table = opt + u16_at(&d, fh + 16) as usize;
    let (sa, fa) = (u32_at(&d, opt + 32), u32_at(&d, opt + 36));
    let sec = |i: usize| sec_table + 40 * i;
    let last = sec(nsec - 1);
    let first_raw = (0..nsec).map(|i| u32_at(&d, sec(i) + 20)).filter(|&p| p != 0).min().unwrap() as usize;
    let hdr = sec(nsec);
    if hdr + 40 > (u32_at(&d, opt + 60) as usize).min(first_raw) || d[hdr..hdr + 40].iter().any(|&b| b != 0) {
        return Err("no room for another section header".into());
    }
    if d.len() != (u32_at(&d, last + 20) + u32_at(&d, last + 16)) as usize {
        return Err("image has overlay data".into());
    }

    // Existing descriptors (without the terminator).
    let (idir_rva, idir_size) = (u32_at(&d, opt + 104), u32_at(&d, opt + 108));
    let pe = parse(exe)?;
    let old = pe.section_table().pe_data_at(exe, idir_rva).ok_or("import directory not in a section")?;
    let old: Vec<&[u8; 20]> = old[..idir_size as usize]
        .as_chunks::<20>()
        .0
        .iter()
        .take_while(|c| c.iter().any(|&b| b != 0))
        .collect();

    let va = align(u32_at(&d, last + 12) + u32_at(&d, last + 8), sa);
    let n = old.len() + 2; // ours + old + terminator
    let mut blob = vec![0u8; 20 * n];
    let mut put = |data: &[u8], alignment: usize| {
        while !blob.len().is_multiple_of(alignment) {
            blob.push(0);
        }
        let rva = va + blob.len() as u32;
        blob.extend_from_slice(data);
        rva
    };
    let name = put(&[dll.as_bytes(), b"\0"].concat(), 2);
    let hint_name = put(&[&[0u8, 0][..], func.as_bytes(), b"\0"].concat(), 2);
    let thunks = [hint_name.to_le_bytes(), [0; 4]].concat();
    let ilt = put(&thunks, 4);
    let iat = put(&thunks, 4);
    for (i, v) in [ilt, 0, 0, name, iat].into_iter().enumerate() {
        put32(&mut blob, 4 * i, v);
    }
    for (i, desc) in old.iter().enumerate() {
        blob[20 * (i + 1)..20 * (i + 2)].copy_from_slice(*desc);
    }

    let raw_ptr = align(d.len() as u32, fa);
    let raw_size = align(blob.len() as u32, fa);
    d[hdr..hdr + 8].copy_from_slice(IMPORT_SECTION);
    put32(&mut d, hdr + 8, blob.len() as u32);
    put32(&mut d, hdr + 12, va);
    put32(&mut d, hdr + 16, raw_size);
    put32(&mut d, hdr + 20, raw_ptr);
    put32(&mut d, hdr + 36, 0xC000_0040); // initialised data, read/write (the loader writes the IAT)
    d[fh + 2..fh + 4].copy_from_slice(&(nsec as u16 + 1).to_le_bytes());
    put32(&mut d, opt + 56, align(va + blob.len() as u32, sa)); // SizeOfImage
    put32(&mut d, opt + 64, 0); // CheckSum (not checked for user-mode images)
    put32(&mut d, opt + 104, va); // DataDirectory[IMPORT]
    put32(&mut d, opt + 108, 20 * n as u32);
    d.resize(raw_ptr as usize, 0);
    d.extend(&blob);
    d.resize((raw_ptr + raw_size) as usize, 0);

    if !imports(&d, dll)? {
        return Err("internal: import table rewrite failed".into());
    }
    Ok(d)
}
