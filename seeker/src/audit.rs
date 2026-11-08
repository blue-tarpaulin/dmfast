//! Checks a patched DLL against its stock copy, byte by byte: the changes are the sites' jumps, the header fields
//! for the two appended sections, and those sections: the code section holds exactly the manifest, the public gates,
//! copies of the sites' original bytes, the bodies (each passing the `validate` rules), and the relocation table;
//! the data section holds the patches' constants (data, never run) and zeros.

use std::ops::Range;

use crate::apply::{check_site, data_vsize, original, overlap, relocations, write_site};
use crate::format::{CODE_SECTION, DATA_SECTION, ExeId, Manifest};
use crate::gate;
use crate::patch::Dll;
use crate::pe::{self, CODE_FLAGS, DATA_FLAGS, DIR_BASERELOC, Pe, align, u32_at};
use crate::validate;

/// How far past an export a site may be and still be named after it.
const NEAR_EXPORT: u32 = 0x400;

/// The identity of dreamseeker.exe that the gates open for.
pub fn exe_id(exe: &[u8]) -> Result<ExeId, String> {
    let pe = Pe::parse(exe)?;
    let field = |o| u32_at(exe, o).ok_or("not a PE32 image");
    Ok(ExeId { timestamp: field(pe.at.file + 4)?, image_size: field(pe.at.image_size())? })
}

/// Ok: a report of what each patch does. Err: every way `installed` differs from what the patches allow.
pub fn audit(installed: &[u8], stock_bytes: &[u8], dll: Dll, exe: ExeId) -> Result<String, String> {
    let file = dll.file();
    let stock = Pe::parse(stock_bytes).map_err(|e| format!("{file}'s backup: {e}"))?;
    let pe = Pe::parse(installed).map_err(|e| format!("{file}: {e}"))?;
    if stock.section(&CODE_SECTION).is_some() {
        return Err(format!("{file}'s backup is itself patched"));
    }
    let n = stock.sections.len();
    if pe.sections.len() != n + 2 || pe.sections[..n] != stock.sections[..] {
        return Err(format!("{file}: its sections differ from the backup's"));
    }
    let (code, data) = (pe.sections[n], pe.sections[n + 1]);
    let mut problems = Vec::new();
    let mut problem = |p: String| problems.push(format!("{file}: {p}"));
    let bad_layout = || format!("{file}: the appended sections aren't laid out as the patcher writes them");

    // The appended sections, exactly as `pe::append` writes them; the manifest says where the constants end.
    let code_end = code.raw_ptr + code.raw_size;
    if code.name != CODE_SECTION
        || code.flags != CODE_FLAGS
        || code.rva != stock.next_rva()
        || code.raw_ptr != align(stock_bytes.len() as u32, stock.file_align)
        || code.raw_size != align(code.vsize, stock.file_align)
        || installed.len() < code_end as usize
    {
        return Err(bad_layout());
    }
    let code_bytes = &installed[code.raw_ptr as usize..(code.raw_ptr + code.vsize) as usize];
    let m = Manifest::parse(code_bytes).ok_or_else(|| format!("{file}: no readable manifest"))?;
    let consts_len = m.data_rva.wrapping_sub(data.rva);
    let data_raw = if consts_len == 0 { (0, 0) } else { (code_end, align(consts_len, stock.file_align)) };
    if data.name != DATA_SECTION
        || data.flags != DATA_FLAGS
        || data.rva != align(code.rva + code.vsize, stock.section_align)
        || consts_len > data.vsize
        || !consts_len.is_multiple_of(4)
        || data.vsize != data_vsize(&m, data.rva)
        || (data.raw_ptr, data.raw_size) != data_raw
        || installed.len() != (code_end + data_raw.1) as usize
    {
        return Err(bad_layout());
    }
    let tail = installed[stock_bytes.len()..code.raw_ptr as usize]
        .iter()
        .chain(&installed[(code.raw_ptr + code.vsize) as usize..code_end as usize]);
    if tail.into_iter().any(|&b| b != 0) {
        problem("bytes around the code section that aren't padding".into());
    }
    if m.exe != exe {
        problem(format!(
            "the gates open for another program (timestamp {:#010x}, image size {:#x}), not this dreamseeker.exe",
            m.exe.timestamp, m.exe.image_size
        ));
    }
    let sites: Vec<_> = m.entries.iter().map(|e| (e.site, e.displace)).collect();
    if let Some((i, j)) = overlap(&sites) {
        problem(format!("{}'s site overlaps {}'s", m.entries[i].name, m.entries[j].name));
    }

    // The original bytes and headers: the stock file with the sites written, apart from the header fields the
    // appended sections change.
    let mut expect = stock_bytes.to_vec();
    for e in &m.entries {
        if let Err(err) = write_site(&stock, &mut expect, e) {
            problem(format!("{}: {err}", e.name));
        }
    }
    let at = stock.at;
    let header_fields = [
        at.sections()..at.sections() + 2,
        at.image_size()..at.image_size() + 4,
        at.checksum()..at.checksum() + 4,
        at.dir(DIR_BASERELOC)..at.dir(DIR_BASERELOC) + 8,
        at.table + 40 * n..at.table + 40 * (n + 2),
    ];
    if let Some(o) =
        (0..stock_bytes.len()).find(|&o| installed[o] != expect[o] && !header_fields.iter().any(|r| r.contains(&o)))
    {
        problem(format!("byte at file offset {o:#x} differs from the backup and isn't a site's jump"));
    }
    let size = u32_at(installed, at.image_size());
    // Relocation and line-number pointers and counts: unused in an image, always 0 from the patcher.
    let unused = (n..n + 2).any(|k| installed[at.table + 40 * k + 24..at.table + 40 * k + 36].iter().any(|&b| b != 0));
    if u32_at(installed, at.checksum()) != Some(0)
        || size != Some(align(data.rva + data.vsize, stock.section_align))
        || unused
    {
        problem("header fields don't match the appended sections".into());
    }

    // The code section, rebuilt from the manifest and the backup with only the bodies taken as they are.
    let mut rebuilt = vec![0xCC; code_bytes.len()];
    let mut used: Vec<(Range<usize>, String)> = Vec::new();
    let mut put = |rva: u32, bytes: &[u8], what: String, problem: &mut dyn FnMut(String)| {
        let o = rva.wrapping_sub(code.rva) as usize;
        match rebuilt.get_mut(o..o + bytes.len()) {
            Some(dst) => {
                dst.copy_from_slice(bytes);
                if let Some((_, other)) = used.iter().find(|(r, _)| r.start < o + bytes.len() && o < r.end) {
                    problem(format!("{what} overlaps {other}"));
                }
                used.push((o..o + bytes.len(), what));
            }
            None => problem(format!("{what} is outside the code section")),
        }
    };
    put(code.rva, &m.encode(), "the manifest".into(), &mut problem);
    let sweep = stock.sweep();
    let count = match m.entries.len() {
        1 => "1 patch".to_string(),
        n => format!("{n} patches"),
    };
    let mut report = vec![format!(
        "{file}: {count} for dreamseeker.exe (timestamp {:#010x}, image size {:#x}).",
        exe.timestamp, exe.image_size
    )];
    for e in &m.entries {
        if let Err(err) = check_site(&stock, &sweep, e.site, e.displace) {
            problem(format!("{}: {err}", e.name));
        }
        put(e.gate, &gate::build(exe, e.gate, e.body, e.orig), format!("{}'s gate", e.name), &mut problem);
        match original(&stock, e) {
            Ok(b) => put(e.orig, &b, format!("{}'s original instructions", e.name), &mut problem),
            Err(err) => problem(format!("{}: {err}", e.name)),
        }
        let Some(body) = pe.bytes(e.body, e.body_len).filter(|_| code.span().contains(&e.body)) else {
            problem(format!("{}: its body is outside the code section", e.name));
            continue;
        };
        put(e.body, body, format!("{}'s body", e.name), &mut problem);
        let consts = pe.bytes(e.consts, e.consts_len);
        let checked = validate::check(&stock, &sweep, &m, e, body);
        for p in checked.problems {
            problem(p);
        }
        let site = match stock.export_before(e.site) {
            Some((_, rva)) if rva == e.site => {
                let names: Vec<_> = stock.exports_at(rva).collect();
                let more =
                    if names.len() > 1 { format!(" and {} more exports", names.len() - 1) } else { String::new() };
                format!("+{:#x} ({}{more})", e.site, names[0])
            }
            Some((name, rva)) if e.site - rva < NEAR_EXPORT => format!("+{:#x} ({name}+{:#x})", e.site, e.site - rva),
            _ => format!("+{:#x}", e.site),
        };
        report.push(format!("{}: {}", e.name, e.about));
        report.push(format!("  replaces {} bytes at {site} with a {}-byte body", e.displace, e.body_len));
        report.extend(checked.report.into_iter().map(|l| format!("  {l}")));
        if let Some(consts) = consts.filter(|c| !c.is_empty()) {
            let text = String::from_utf8_lossy(consts.strip_suffix(&[0]).unwrap_or(consts)).escape_debug().to_string();
            report.push(format!("  constants: \"{text}\""));
        }
    }

    // The data section: each patch's constants, taken as they are, and zeros.
    let mut data_bytes = vec![0u8; data_raw.1 as usize];
    let mut spans: Vec<Range<u32>> = Vec::new();
    for e in m.entries.iter().filter(|e| e.consts_len > 0) {
        let at = e.consts.wrapping_sub(data.rva);
        let span = at..at.saturating_add(e.consts_len);
        match pe.bytes(e.consts, e.consts_len) {
            Some(c) if span.end <= consts_len && !spans.iter().any(|s| s.start < span.end && span.start < s.end) => {
                data_bytes[span.start as usize..span.end as usize].copy_from_slice(c);
                spans.push(span);
            }
            _ => problem(format!("{}: its constants aren't in the data section's constants", e.name)),
        }
    }
    if installed[code_end as usize..] != data_bytes[..] {
        problem("the data section holds bytes that aren't a patch's constants".into());
    }
    let (reloc_rva, reloc_len) = pe.dir(DIR_BASERELOC);
    let table = pe::reloc_table(&relocations(&stock, &m.entries));
    if reloc_len as usize != table.len() {
        problem(
            "the relocation table isn't the backup's (sites' moved to their copies) plus the bodies' addresses".into(),
        );
    }
    put(reloc_rva, &table, "the relocation table".into(), &mut problem);
    if let Some(o) = (0..code_bytes.len()).find(|&o| code_bytes[o] != rebuilt[o]) {
        problem(format!("byte at +{:#x} in the code section isn't part of any patch", code.rva as usize + o));
    }

    if problems.is_empty() {
        report.push(format!("{file}: Validation complete, modified bytes are from patches and follow the rules."));
        Ok(report.join("\n"))
    } else {
        Err(problems.join("\n"))
    }
}
