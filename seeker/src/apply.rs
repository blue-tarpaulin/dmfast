//! Placing patches in a stock DLL: the sites become jumps to gates; the manifest, gates, original instructions,
//! bodies, and the new relocation table go into an appended code section; and each patch's constants and data into
//! an appended data section.

use std::collections::BTreeSet;

use iced_x86::{Decoder, DecoderOptions, OpKind};

use crate::audit;
use crate::format::{CODE_SECTION, Class, DATA_SECTION, Entry, ExeId, Field, Fixup, JMP_LEN, Manifest};
use crate::gate;
use crate::patch::{Dll, Patch, Place, Ref};
use crate::pe::{self, CODE_FLAGS, DATA_FLAGS, DIR_BASERELOC, NewSection, Pe, Sweep, align, put32};
use crate::validate;

/// Each body starts on this boundary.
const CODE_ALIGN: u32 = 16;

pub struct Applied {
    /// The patched DLL; `None` if no patch for it applies.
    pub image: Option<Vec<u8>>,
    /// One line per patch for this DLL: where it was applied, or why it was skipped.
    pub lines: Vec<String>,
}

/// What a fixup points at, before the sections are laid out.
enum Target {
    Fixed(Class, u32),
    Body(u32),
    Data(u32),
    Const(u32),
    Original,
}

struct Placed<'a> {
    p: &'a Patch,
    site: u32,
    refs: Vec<(u32, Field, Target)>,
}

/// Applies the patches for `dll` to its stock bytes. A patch that can't be placed on this build or breaks a rule is
/// skipped (with a line saying why); the result passes [`audit::audit`].
pub fn apply(stock_bytes: &[u8], dll: Dll, exe: ExeId, patches: &[Patch]) -> Result<Applied, String> {
    let stock = Pe::parse(stock_bytes)?;
    if stock.section(&CODE_SECTION).is_some() {
        return Err(format!("{} already has DreamSeeker patches", dll.file()));
    }
    let sweep = stock.sweep();
    let mut lines = Vec::new();
    let mut placed: Vec<Placed> = Vec::new();
    for p in patches.iter().filter(|p| p.dll == dll) {
        match place(&stock, &sweep, p, &placed) {
            Ok(s) => placed.push(s),
            Err(e) => lines.push(format!("{}: skipped ({e})", p.name)),
        }
    }
    loop {
        if placed.is_empty() {
            return Ok(Applied { image: None, lines });
        }
        let (image, m) = build(&stock, exe, &placed)?;
        let pe = Pe::parse(&image)?;
        let failed: Vec<(usize, String)> = m
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let Some(body) = pe.bytes(e.body, e.body_len) else { return Some((i, "body unreadable".into())) };
                let c = validate::check(&stock, &sweep, &m, e, body);
                (!c.problems.is_empty()).then(|| (i, c.problems.join("; ")))
            })
            .collect();
        if failed.is_empty() {
            audit::audit(&image, stock_bytes, dll, exe)
                .map_err(|e| format!("internal: the patched {} fails its audit:\n{e}", dll.file()))?;
            lines.extend(m.entries.iter().map(|e| format!("{}: applied at +{:#x}", e.name, e.site)));
            return Ok(Applied { image: Some(image), lines });
        }
        for (i, why) in failed.into_iter().rev() {
            let s = placed.remove(i);
            lines.push(format!("{}: skipped ({why})", s.p.name));
        }
    }
}

fn resolve(stock: &Pe, place: &Place) -> Result<u32, String> {
    match place {
        Place::Export(name) => stock.export(name).ok_or_else(|| format!("export {name} not found")),
        Place::Pattern(ida, offset) => Ok(stock.find_pattern(ida)?.wrapping_add_signed(*offset)),
    }
}

/// Checks that `displace` bytes at `site` can become a jump to a gate and run elsewhere unchanged: at least 5 bytes
/// of whole instructions in the code, starting on one, none relative, no relocated address across their edges, and
/// nothing found branching into them.
pub(crate) fn check_site(stock: &Pe, sweep: &Sweep, site: u32, displace: u32) -> Result<(), String> {
    if displace < JMP_LEN {
        return Err(format!("displace {displace} is less than a jmp's 5 bytes"));
    }
    if !stock.is_code(site) || !stock.is_code(site + displace - 1) {
        return Err(format!("site +{site:#x} isn't in the DLL's code"));
    }
    if !sweep.starts_at(site) {
        return Err(format!("site +{site:#x} isn't the start of an instruction"));
    }
    let code = (1..=displace + 15)
        .rev()
        .find_map(|n| stock.bytes(site, n))
        .ok_or_else(|| format!("site +{site:#x} isn't in the file"))?;
    let mut dec = Decoder::with_ip(32, code, u64::from(site), DecoderOptions::NONE);
    let mut len = 0;
    while len < displace {
        let ins = dec.decode();
        if ins.is_invalid() {
            return Err(format!("site +{site:#x} doesn't decode"));
        }
        if ins.op_kinds().any(|k| {
            matches!(k, OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::FarBranch16 | OpKind::FarBranch32)
        }) {
            return Err(format!("site +{:#x} has a relative branch", ins.ip32()));
        }
        len += ins.len() as u32;
    }
    if len != displace {
        return Err(format!("displace {displace} ends inside an instruction (whole instructions: {len})"));
    }
    // A relocated address inside the site moves to the copy of its original instructions; one across its edge can't.
    if let Some(r) =
        stock.relocs.range(site.saturating_sub(3)..site + displace).find(|&&r| r < site || r + 4 > site + displace)
    {
        return Err(format!("a relocated address at +{r:#x} crosses the site's edge"));
    }
    if let Some(r) = sweep.branched_into(site + 1..site + displace) {
        return Err(format!("code branches to +{r:#x}, inside the site"));
    }
    Ok(())
}

/// The first two of `sites` (address, length) that overlap: the later one's index, then the earlier one's.
pub(crate) fn overlap(sites: &[(u32, u32)]) -> Option<(usize, usize)> {
    let crosses = |a: (u32, u32), b: (u32, u32)| a.0 < b.0 + b.1 && b.0 < a.0 + a.1;
    (0..sites.len()).find_map(|i| (0..i).find(|&j| crosses(sites[i], sites[j])).map(|j| (i, j)))
}

fn place<'a>(stock: &Pe, sweep: &Sweep, p: &'a Patch, placed: &[Placed]) -> Result<Placed<'a>, String> {
    if p.body.is_empty() {
        return Err("its body is empty".into());
    }
    let site = resolve(stock, &p.site)?;
    check_site(stock, sweep, site, p.displace)?;
    let sites: Vec<_> = placed.iter().map(|o| (o.site, o.p.displace)).chain([(site, p.displace)]).collect();
    if let Some((_, j)) = overlap(&sites) {
        return Err(format!("its site overlaps {}'s", placed[j].p.name));
    }
    let refs = p
        .fixups
        .iter()
        .map(|f| {
            if f.at.checked_add(4).is_none_or(|end| end as usize > p.body.len()) {
                return Err(format!("fixup at +{:#x} is outside the body", f.at));
            }
            let t = match &f.target {
                Ref::Body(o) => Target::Body(*o),
                Ref::Data(o) if *o < p.data => Target::Data(*o),
                Ref::Data(o) => return Err(format!("data offset {o:#x} is past its {} bytes", p.data)),
                Ref::Const(o) if (*o as usize) < p.consts.len() => Target::Const(*o),
                Ref::Const(o) => return Err(format!("constant offset {o:#x} is past its {} bytes", p.consts.len())),
                Ref::Resume => Target::Fixed(Class::Resume, site + p.displace),
                Ref::Original => Target::Original,
                Ref::Code(place) => Target::Fixed(Class::Code, resolve(stock, place)?),
                Ref::Global(place) => Target::Fixed(Class::Global, resolve(stock, place)?),
                Ref::Import { dll, name } => Target::Fixed(
                    Class::Import,
                    stock.import_slot(dll, name).ok_or_else(|| format!("the DLL doesn't import {dll}!{name}"))?,
                ),
            };
            Ok((f.at, f.field, t))
        })
        .collect::<Result<_, String>>()?;
    Ok(Placed { p, site, refs })
}

/// The patched image and its manifest.
fn build(stock: &Pe, exe: ExeId, placed: &[Placed]) -> Result<(Vec<u8>, Manifest), String> {
    let code_rva = stock.next_rva();
    let entries = placed
        .iter()
        .map(|s| Entry {
            name: s.p.name.clone(),
            about: s.p.about.clone(),
            site: s.site,
            displace: s.p.displace,
            gate: 0,
            orig: 0,
            body: 0,
            body_len: s.p.body.len() as u32,
            consts: 0,
            consts_len: s.p.consts.len() as u32,
            fixups: s
                .refs
                .iter()
                .map(|&(_, field, _)| Fixup { field_rva: 0, field, class: Class::Body, target: 0 })
                .collect(),
        })
        .collect();
    let mut m = Manifest { exe, data_rva: 0, data_len: 0, entries };
    // The manifest's length depends only on the counts and strings, which are final.
    let mut off = align(m.encode().len() as u32, CODE_ALIGN);
    for (e, s) in m.entries.iter_mut().zip(placed) {
        e.gate = code_rva + off;
        e.orig = e.gate + gate::LEN;
        e.body = code_rva + align(e.orig - code_rva + e.displace + 5, CODE_ALIGN);
        off = align(e.body - code_rva + e.body_len, CODE_ALIGN);
        for (f, r) in e.fixups.iter_mut().zip(&s.refs) {
            f.field_rva = e.body + r.0;
        }
    }
    let reloc_off = off;
    let relocs = pe::reloc_table(&relocations(stock, &m.entries));
    let code_len = reloc_off + relocs.len() as u32;

    // The data section: each patch's constants, then each patch's zero-filled data.
    let data_rva = align(code_rva + code_len, stock.section_align);
    let mut consts = Vec::new();
    for (e, s) in m.entries.iter_mut().zip(placed) {
        consts.resize(consts.len().next_multiple_of(4), 0);
        e.consts = data_rva + consts.len() as u32;
        consts.extend_from_slice(&s.p.consts);
    }
    consts.resize(consts.len().next_multiple_of(4), 0);
    m.data_rva = data_rva + consts.len() as u32;
    let mut data_offsets = Vec::new();
    for s in placed {
        data_offsets.push(m.data_len);
        m.data_len += align(s.p.data, 4);
    }

    let mut code = vec![0xCC; code_len as usize];
    for ((e, s), data_off) in m.entries.iter_mut().zip(placed).zip(data_offsets) {
        for (f, (at, field, t)) in e.fixups.iter_mut().zip(&s.refs) {
            let (class, target) = match *t {
                Target::Fixed(class, rva) => (class, rva),
                Target::Body(o) => (Class::Body, e.body + o),
                Target::Data(o) => (Class::Data, m.data_rva + data_off + o),
                Target::Const(o) => (Class::Const, e.consts + o),
                Target::Original => (Class::Original, e.orig),
            };
            *f = Fixup { field_rva: e.body + at, field: *field, class, target };
        }
        let at = |rva: u32| (rva - code_rva) as usize;
        code[at(e.gate)..at(e.orig)].copy_from_slice(&gate::build(exe, e.gate, e.body, e.orig));
        code[at(e.orig)..at(e.orig) + e.displace as usize + 5].copy_from_slice(&original(stock, e)?);
        let body = &mut code[at(e.body)..at(e.body) + e.body_len as usize];
        body.copy_from_slice(&s.p.body);
        for f in &e.fixups {
            let v = match f.field {
                Field::Rel32 => f.target.wrapping_sub(f.field_rva + 4),
                Field::Abs32 => stock.base.wrapping_add(f.target),
            };
            put32(body, (f.field_rva - e.body) as usize, v);
        }
    }
    code[reloc_off as usize..].copy_from_slice(&relocs);
    let header = m.encode();
    code[..header.len()].copy_from_slice(&header);

    let sections = [
        NewSection { name: CODE_SECTION, flags: CODE_FLAGS, data: &code, vsize: 0 },
        NewSection { name: DATA_SECTION, flags: DATA_FLAGS, data: &consts, vsize: data_vsize(&m, data_rva) },
    ];
    let mut image = pe::append(stock, &sections, &[(DIR_BASERELOC, code_rva + reloc_off, relocs.len() as u32)])?;
    for e in &m.entries {
        write_site(stock, &mut image, e)?;
    }
    Ok((image, m))
}

/// The data section's size in memory: the constants, then the zero-filled data (at least 4 bytes).
pub(crate) fn data_vsize(m: &Manifest, data_rva: u32) -> u32 {
    (m.data_rva - data_rva + m.data_len).max(4)
}

/// The site's original bytes, then a `jmp` back to the instruction after the site.
pub(crate) fn original(stock: &Pe, e: &Entry) -> Result<Vec<u8>, String> {
    let site = stock.bytes(e.site, e.displace).ok_or_else(|| format!("site +{:#x} isn't in the file", e.site))?;
    Ok([site, &e.jump_back()].concat())
}

/// Writes `e`'s site into `image`: a `jmp` to its gate, then `int3`s.
pub(crate) fn write_site(stock: &Pe, image: &mut [u8], e: &Entry) -> Result<(), String> {
    let bytes = e.site_bytes();
    let dst = stock
        .offset(e.site)
        .filter(|_| e.displace >= JMP_LEN && stock.bytes(e.site, e.displace).is_some())
        .and_then(|o| image.get_mut(o..o + bytes.len()))
        .ok_or_else(|| format!("site +{:#x} ({} bytes) can't hold a jump in the file", e.site, e.displace))?;
    dst.copy_from_slice(&bytes);
    Ok(())
}

/// The patched DLL's relocated fields: the stock ones, with those inside a site moved to the same place in its copy
/// of the original instructions, and every body's `Abs32` field.
pub(crate) fn relocations(stock: &Pe, entries: &[Entry]) -> BTreeSet<u32> {
    let site_of = |r: u32| entries.iter().find(|e| (e.site..e.site + e.displace).contains(&r));
    let moved = stock.relocs.iter().map(|&r| site_of(r).map_or(r, |e| e.orig + (r - e.site)));
    let fields = entries.iter().flat_map(|e| &e.fixups).filter(|f| f.field == Field::Abs32).map(|f| f.field_rva);
    moved.chain(fields).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::Fixup as PatchFixup;

    const BASE: u32 = 0x1000_0000;
    const EXE: ExeId = ExeId { timestamp: 0x1234_5678, image_size: 0xD_0000 };

    /// A small PE32 DLL: `.text` (exports `site`, `helper`, `?Login@X@@QAEXXZ`, `relocsite`, and `site2`, an import
    /// thunk and a call to it, and a jump into `site2`), `.rdata` (imports `byondcore.dll!Foo` and
    /// `kernel32.dll!ExitProcess`, the export table, an exported `global`, and an exported `slot` in the IAT), and
    /// `.reloc`.
    fn dll() -> Vec<u8> {
        let mut d = vec![0u8; 0xA00];
        let put = |d: &mut Vec<u8>, o: usize, b: &[u8]| d[o..o + b.len()].copy_from_slice(b);
        let p32 = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0, b"MZ");
        p32(&mut d, 0x3C, 0x40);
        put(&mut d, 0x40, b"PE\0\0");
        put(&mut d, 0x44, &[0x4C, 0x01, 3, 0]); // i386, 3 sections
        put(&mut d, 0x54, &[0xE0, 0, 0x02, 0x21]); // optional header size, DLL
        let opt = 0x58;
        put(&mut d, opt, &[0x0B, 0x01]);
        for (o, v) in [(28, BASE), (32, 0x1000), (36, 0x200), (56, 0x4000), (60, 0x400), (92, 16)] {
            p32(&mut d, opt + o, v);
        }
        for (i, rva, size) in [(0, 0x2100, 0xE8), (1, 0x2000, 60), (5, 0x3000, 12), (12, 0x20A0, 16)] {
            p32(&mut d, opt + 96 + 8 * i, rva);
            p32(&mut d, opt + 100 + 8 * i, size);
        }
        let table = opt + 0xE0;
        for (i, (name, rva, vsize, raw, flags)) in [
            (b".text\0\0\0", 0x1000, 0x100, 0x400, 0x6000_0020),
            (b".rdata\0\0", 0x2000, 0x200, 0x600, 0x4000_0040),
            (b".reloc\0\0", 0x3000, 0x10, 0x800, 0x4200_0040),
        ]
        .into_iter()
        .enumerate()
        {
            let h = table + 40 * i;
            put(&mut d, h, name);
            for (o, v) in [(8, vsize), (12, rva), (16, 0x200), (20, raw), (36, flags)] {
                p32(&mut d, h + o, v);
            }
        }
        let text = |rva: u32| (rva - 0x1000 + 0x400) as usize;
        d[text(0x1000)..text(0x1100)].fill(0xCC);
        put(&mut d, text(0x1000), &[0xC7, 0x01, 0, 0, 0, 0, 0x8B, 0xC1, 0xC3]); // site
        put(&mut d, text(0x1010), &[0x55, 0x8B, 0xEC, 0x5D, 0xC3]); // helper
        put(&mut d, text(0x1020), &[0xC3]); // ?Login@X@@QAEXXZ
        put(&mut d, text(0x1030), &[0xA1, 0xF0, 0x21, 0x00, 0x10, 0xC3]); // relocsite: mov eax, [global]
        put(&mut d, text(0x1040), &[0xFF, 0x25, 0xA0, 0x20, 0x00, 0x10]); // jmp [Foo's slot]
        put(&mut d, text(0x1050), &[0xE9, 0x0D, 0, 0, 0]); // jmp site2+2
        put(&mut d, text(0x1060), &[0xC7, 0x01, 0, 0, 0, 0, 0xC3]); // site2
        put(&mut d, text(0x1070), &[0xE8, 0xCB, 0xFF, 0xFF, 0xFF]); // call the thunk
        let rdata = |rva: u32| (rva - 0x2000 + 0x600) as usize;
        for (k, (ilt, name, iat)) in [(0x2080, 0x20C0, 0x20A0), (0x2088, 0x20D0, 0x20A8)].into_iter().enumerate() {
            for (o, v) in [(0, ilt), (12, name), (16, iat)] {
                p32(&mut d, rdata(0x2000) + 20 * k + o, v);
            }
        }
        for (slot, hint) in [(0x2080, 0x20E0), (0x2088, 0x20F0), (0x20A0, 0x20E0), (0x20A8, 0x20F0)] {
            p32(&mut d, rdata(slot), hint);
        }
        put(&mut d, rdata(0x20C0), b"byondcore.dll");
        put(&mut d, rdata(0x20D0), b"kernel32.dll");
        put(&mut d, rdata(0x20E2), b"Foo");
        put(&mut d, rdata(0x20F2), b"ExitProcess");
        for (o, v) in [(12, 0x2190), (16, 1), (20, 7), (24, 7), (28, 0x2140), (32, 0x2160), (36, 0x2180)] {
            p32(&mut d, rdata(0x2100) + o, v);
        }
        let exports: [(u32, u32, &[u8]); 7] = [
            (0x1000, 0x21A0, b"site"),
            (0x1010, 0x21A8, b"helper"),
            (0x1020, 0x21B0, b"?Login@X@@QAEXXZ"),
            (0x1030, 0x21C8, b"relocsite"),
            (0x1060, 0x21D8, b"site2"),
            (0x21F0, 0x21E0, b"global"),
            (0x20A0, 0x21F8, b"slot"),
        ];
        for (i, (rva, name, text)) in exports.into_iter().enumerate() {
            p32(&mut d, rdata(0x2140) + 4 * i, rva);
            p32(&mut d, rdata(0x2160) + 4 * i, name);
            put(&mut d, rdata(0x2180) + 2 * i, &(i as u16).to_le_bytes());
            put(&mut d, rdata(name), text);
        }
        put(&mut d, rdata(0x2190), b"test.dll");
        // .reloc: the two absolute addresses in .text.
        put(&mut d, 0x800, &[0x00, 0x10, 0, 0, 12, 0, 0, 0, 0x31, 0x30, 0x42, 0x30]);
        d
    }

    fn patch(site: &str, displace: u32, body: &[u8], fixups: &[(u32, Field, Ref)]) -> Patch {
        Patch {
            key: "p".into(),
            tier: crate::patch::Tier::Safe,
            name: "p".into(),
            about: "a test patch".into(),
            dll: Dll::Core,
            site: Place::Export(site.into()),
            displace,
            body: body.to_vec(),
            fixups: fixups
                .iter()
                .map(|(at, field, target)| PatchFixup { at: *at, field: *field, target: target.clone() })
                .collect(),
            data: 4,
            consts: Vec::new(),
        }
    }

    /// `lock inc dword ptr [data]`, then `jmp original`.
    fn counter() -> Patch {
        patch(
            "site",
            6,
            &[0xF0, 0xFF, 0x05, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0],
            &[(3, Field::Abs32, Ref::Data(0)), (8, Field::Rel32, Ref::Original)],
        )
    }

    fn applied(p: Patch) -> Vec<u8> {
        let a = apply(&dll(), Dll::Core, EXE, &[p]).unwrap();
        a.image.unwrap_or_else(|| panic!("not applied: {:?}", a.lines))
    }

    fn skipped(p: Patch, why: &str) {
        let a = apply(&dll(), Dll::Core, EXE, &[p]).unwrap();
        assert!(a.image.is_none() && a.lines[0].contains(why), "expected {why:?}, got {:?}", a.lines);
    }

    #[test]
    fn applies_and_audits() {
        let stock = dll();
        let image = applied(counter());
        let report = audit::audit(&image, &stock, Dll::Core, EXE).unwrap();
        assert!(
            report.contains("replaces 6 bytes at +0x1000 (site) with a 12-byte body")
                && report.contains("data it uses: its own data"),
            "{report}"
        );
        let pe = Pe::parse(&image).unwrap();
        assert_eq!(pe.relocs, stock_relocs_plus(&pe));
        // The site jumps to the gate, which opens for EXE.
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        let e = &m.entries[0];
        assert_eq!(pe.bytes(0x1000, 6).unwrap(), [&[0xE9][..], &(e.gate - 0x1005).to_le_bytes(), &[0xCC]].concat());
        assert_eq!(pe.bytes(e.gate, gate::LEN).unwrap(), gate::build(EXE, e.gate, e.body, e.orig));
        assert_eq!(pe.bytes(e.orig, 6).unwrap(), [0xC7, 0x01, 0, 0, 0, 0]);
    }

    fn stock_relocs_plus(pe: &Pe) -> BTreeSet<u32> {
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(pe.data)).unwrap();
        let fields = m.entries.iter().flat_map(|e| &e.fixups).filter(|f| f.field == Field::Abs32).map(|f| f.field_rva);
        [0x1031, 0x1042].into_iter().chain(fields).collect()
    }

    #[test]
    fn site_with_a_relocated_address() {
        let stock = dll();
        let image = applied(patch("relocsite", 5, &[0xE9, 0, 0, 0, 0], &[(1, Field::Rel32, Ref::Original)]));
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        let orig = m.entries[0].orig;
        // The site's address is relocated in the copy now, not in the site (which is a jmp).
        assert_eq!(pe.relocs, BTreeSet::from([orig + 1, 0x1042]));
        assert_eq!(pe.bytes(orig, 5).unwrap(), [0xA1, 0xF0, 0x21, 0x00, 0x10]);
        audit::audit(&image, &stock, Dll::Core, EXE).unwrap();
        let mut inside = patch("site", 5, &[0xC3], &[]);
        inside.site = Place::Pattern("21 00 10 C3".into(), 0);
        skipped(inside, "isn't the start of an instruction");
    }

    #[test]
    fn overlapping_sites() {
        assert_eq!(overlap(&[(0x1000, 6), (0x1010, 5), (0x1005, 5)]), Some((2, 0)));
        assert_eq!(overlap(&[(0x1000, 6), (0x1006, 5)]), None);
        let mut second = counter();
        second.name = "q".into();
        let a = apply(&dll(), Dll::Core, EXE, &[counter(), second]).unwrap();
        assert!(a.lines.iter().any(|l| l.contains("q: skipped (its site overlaps p's)")), "{:?}", a.lines);
    }

    #[test]
    fn constants() {
        // push offset text ; pop eax ; jmp original
        let mut p = patch(
            "site",
            6,
            &[0x68, 0, 0, 0, 0, 0x58, 0xE9, 0, 0, 0, 0],
            &[(1, Field::Abs32, Ref::Const(0)), (7, Field::Rel32, Ref::Original)],
        );
        p.consts = b"hi\0".to_vec();
        let image = applied(p.clone());
        let report = audit::audit(&image, &dll(), Dll::Core, EXE).unwrap();
        assert!(report.contains("constants: \"hi\""), "{report}");
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        assert_eq!(pe.bytes(m.entries[0].consts, 3).unwrap(), b"hi\0");
        assert!(!pe.is_code(m.entries[0].consts), "constants must not be executable");
        p.fixups[0].target = Ref::Const(3);
        skipped(p, "past its 3 bytes");
    }

    #[test]
    fn audit_catches_tampering() {
        let stock = dll();
        let image = applied(counter());
        let pe = Pe::parse(&image).unwrap();
        let code = *pe.section(&CODE_SECTION).unwrap();
        let m = Manifest::parse(code.bytes(&image)).unwrap();
        let fails = |img: &[u8], exe: ExeId, why: &str| {
            let err = audit::audit(img, &stock, Dll::Core, exe).unwrap_err();
            assert!(err.contains(why), "expected {why:?}, got {err}");
        };
        let mut t = image.clone();
        t[0x400 + 0x12] ^= 1; // helper
        fails(&t, EXE, "differs from the backup");
        let mut t = image.clone();
        t[code.raw_ptr as usize + (m.entries[0].gate - code.rva) as usize - 1] = 0x90; // padding before the gate
        fails(&t, EXE, "isn't part of any patch");
        let mut t = image.clone();
        t[code.raw_ptr as usize + (m.entries[0].body - code.rva) as usize + 7] = 0xE8; // jmp original -> call original
        fails(&t, EXE, "doesn't end in a jmp or ret");
        fails(&image, ExeId { timestamp: 1, ..EXE }, "another program");
    }

    #[test]
    fn site_rules() {
        skipped(patch("site", 4, &[0xC3], &[]), "less than");
        skipped(patch("site", 7, &[0xC3], &[]), "ends inside an instruction");
        skipped(patch("site2", 6, &[0xC3], &[]), "inside the site");
        skipped(patch("nothing", 6, &[0xC3], &[]), "not found");
    }

    #[test]
    fn body_rules() {
        let jmp_orig = |prefix: &[u8], fixups: &[(u32, Field, Ref)]| {
            let mut body = prefix.to_vec();
            body.extend_from_slice(&[0xE9, 0, 0, 0, 0]);
            let mut f = fixups.to_vec();
            f.push((prefix.len() as u32 + 1, Field::Rel32, Ref::Original));
            patch("site", 6, &body, &f)
        };
        let call = |place: Place| [(1, Field::Rel32, Ref::Code(place))];
        let slot = |dll: &str, name: &str| [(2, Field::Abs32, Ref::Import { dll: dll.into(), name: name.into() })];
        let e8 = [0xE8, 0, 0, 0, 0];
        let ff15 = [0xFF, 0x15, 0, 0, 0, 0];
        // Allowed: calling an exported function, an import slot of a BYOND DLL, and reading BYOND's data.
        applied(jmp_orig(&e8, &call(Place::Export("helper".into()))));
        applied(jmp_orig(&ff15, &slot("byondcore.dll", "Foo")));
        applied(jmp_orig(&[0xA1, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Global(Place::Export("global".into())))]));
        // Not allowed.
        skipped(jmp_orig(&e8, &call(Place::Export("?Login@X@@QAEXXZ".into()))), "denied (login)");
        skipped(jmp_orig(&e8, &call(Place::Pattern("FF 25 A0 20 00 10".into(), 0))), "import thunk");
        // helper+3 is an instruction, but not a function's start.
        skipped(jmp_orig(&e8, &call(Place::Pattern("55 8B EC".into(), 3))), "not the start of a function");
        skipped(
            jmp_orig(&[0xA1, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Global(Place::Export("slot".into())))]),
            "address",
        );
        skipped(jmp_orig(&ff15, &slot("kernel32.dll", "ExitProcess")), "not a BYOND DLL");
        skipped(jmp_orig(&[0xCD, 0x2E], &[]), "interrupts");
        skipped(jmp_orig(&[0x0F, 0x34], &[]), "system calls");
        skipped(jmp_orig(&[0x64, 0xA1, 0x30, 0, 0, 0], &[]), "fs/gs");
        skipped(jmp_orig(&[0x8E, 0xD8], &[]), "segment registers");
        skipped(jmp_orig(&[0xFF, 0xD0], &[]), "indirect branch");
        skipped(jmp_orig(&[0x89, 0xC4], &[]), "changes esp");
        skipped(jmp_orig(&[0xA1, 0xF0, 0x21, 0x00, 0x10], &[]), "absolute address without a fixup");
        skipped(jmp_orig(&[0xEB, 0x01, 0xB8, 0, 0, 0, 0], &[]), "not an instruction of the body");
        skipped(patch("site", 6, &[0x90], &[]), "doesn't end in a jmp or ret");
        skipped(jmp_orig(&[0x68, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Body(0))]), "isn't its own data");
    }
}
