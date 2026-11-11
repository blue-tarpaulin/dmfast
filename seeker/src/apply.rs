//! Placing patches in a stock module: the sites become jumps to gates; the manifest, gates, original instructions,
//! bodies, the Direct3D helper when a patch uses it, and the new relocation table go into an appended code section;
//! and each patch's constants and data, and the helper's cache, into an appended data section.

use std::collections::BTreeSet;

use iced_x86::{Decoder, DecoderOptions, OpKind};

use crate::audit;
use crate::com;
use crate::gate;
use crate::patch::{Module, Patch, Place, Ref};
use crate::pe::{self, CODE_FLAGS, DATA_FLAGS, DIR_BASERELOC, NewSection, Pe, Sweep, align, put32};
use crate::validate;
use dmfast_format::manifest::{CODE_SECTION, Class, DATA_SECTION, Entry, ExeId, Field, Fixup, JMP_LEN, Manifest};

/// Each body, and the Direct3D helper, starts on this boundary.
const CODE_ALIGN: u32 = 16;
/// Each patch's data starts on this boundary (its statics may want it).
const DATA_ALIGN: u32 = 16;

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
    Original(u32),
    Com,
    ComTail,
}

struct Placed<'a> {
    p: &'a Patch,
    site: u32,
    refs: Vec<(u32, Field, Target)>,
}

/// Applies the patches for `module` to its stock bytes. `exe` is dreamseeker.exe's stock bytes, which `exe` patterns
/// are checked against. `id` is the identity the gates check for; pass `None` when patching dreamseeker.exe itself, so
/// that its gates check for the patched file. A patch that can't be placed on this build or breaks a rule is skipped,
/// with a line saying why (no line for a missing site when another patch with the same key, the same fix for another
/// build, applied). The result passes [`audit::audit`].
pub fn apply(
    stock_bytes: &[u8],
    module: Module,
    exe: &[u8],
    id: Option<ExeId>,
    patches: &[Patch],
) -> Result<Applied, String> {
    let stock = Pe::parse(stock_bytes)?;
    if stock.section(&CODE_SECTION).is_some() {
        return Err(format!("{} already has DreamSeeker patches", module.file()));
    }
    let exe_pe = Pe::parse(exe).map_err(|e| format!("dreamseeker.exe: {e}"))?;
    let own = id.is_none();
    let mut id = id.map_or_else(|| audit::exe_id(stock_bytes), Ok)?;
    let sweep = stock.sweep();
    let (mut lines, mut absent) = (Vec::new(), Vec::new());
    let mut placed: Vec<Placed> = Vec::new();
    for p in patches.iter().filter(|p| p.module == module) {
        let site = match p.exe.iter().try_for_each(|ida| exe_pe.find_pattern(ida).map(drop)) {
            Ok(()) => resolve(&stock, &p.site),
            Err(e) => Err(format!("dreamseeker.exe: {e}")),
        };
        let site = match site {
            Ok(site) => site,
            Err(e) => {
                absent.push((&p.key, format!("{}: skipped ({e})", p.name)));
                continue;
            }
        };
        match place(&stock, &sweep, p, site, &placed) {
            Ok(s) => placed.push(s),
            Err(e) => lines.push(format!("{}: skipped ({e})", p.name)),
        }
    }
    loop {
        if placed.is_empty() {
            lines.extend(absent.into_iter().map(|(_, l)| l));
            return Ok(Applied { image: None, lines });
        }
        let (mut image, mut m) = build(&stock, id, &placed)?;
        // The patched exe's size is only known after layout, so lay it out again for its gates to check that size.
        if own && audit::exe_id(&image)? != id {
            id = audit::exe_id(&image)?;
            (image, m) = build(&stock, id, &placed)?;
        }
        let pe = Pe::parse(&image)?;
        let failed: Vec<(usize, String)> = m
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let Some(body) = pe.bytes(e.body, e.body_len) else { return Some((i, "body unreadable".into())) };
                let consts = pe.bytes(e.consts, e.consts_len).unwrap_or_default();
                let c = validate::check(&stock, &sweep, &m, e, body, consts);
                (!c.problems.is_empty()).then(|| (i, c.problems.join("; ")))
            })
            .collect();
        if failed.is_empty() {
            audit::audit(&image, stock_bytes, module, id)
                .map_err(|e| format!("internal: the patched {} fails its audit:\n{e}", module.file()))?;
            lines.extend(absent.iter().filter(|(k, _)| !placed.iter().any(|s| &s.p.key == *k)).map(|(_, l)| l.clone()));
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
        Place::Fingerprint { prefix, len, hash } => {
            let found: Vec<_> =
                stock.find_all(prefix)?.into_iter().map(|at| (at, stock.fingerprint(at, *len))).collect();
            match found.iter().filter(|(_, h)| *h == Some(*hash)).collect::<Vec<_>>()[..] {
                [(at, _)] => Ok(*at),
                [] => {
                    // What each candidate hashes to, for whoever writes the fingerprint.
                    let seen: Vec<_> = found.iter().map(|(at, h)| format!("+{at:#x}: {:#x}", h.unwrap_or(0))).collect();
                    Err(format!("no {len}-byte function after {prefix:?} hashes to {hash:#x} ({})", seen.join(", ")))
                }
                ref many => Err(format!("{} functions match fingerprint {hash:#x}", many.len())),
            }
        }
    }
}

/// Checks that `displace` bytes at `site` can become a jump to a gate and run elsewhere unchanged: at least 5 bytes
/// of whole instructions in the code, starting on one, relative branches only to their end (in the copy of the
/// original instructions, that's the jump back), no relocated address across their edges, and nothing found
/// branching into them.
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
        let branch = ins.op_kinds().any(|k| {
            matches!(k, OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::FarBranch16 | OpKind::FarBranch32)
        });
        // A jump to the site's end lands, in the copy of its original instructions, on the jump back.
        if branch && (ins.is_call_near() || ins.near_branch_target() != u64::from(site + displace)) {
            return Err(format!("site +{:#x} has a relative branch out of the site", ins.ip32()));
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

fn place<'a>(stock: &Pe, sweep: &Sweep, p: &'a Patch, site: u32, placed: &[Placed]) -> Result<Placed<'a>, String> {
    if p.body.is_empty() {
        return Err("its body is empty".into());
    }
    if p.entry as usize >= p.body.len() {
        return Err(format!("its entry +{:#x} is past its {}-byte body", p.entry, p.body.len()));
    }
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
                Ref::Original(o) if *o < p.displace => Target::Original(*o),
                Ref::Original(o) => return Err(format!("original offset {o} is past its {} bytes", p.displace)),
                Ref::Code(place) => Target::Fixed(Class::Code, resolve(stock, place)?),
                Ref::Global(place) => Target::Fixed(Class::Global, resolve(stock, place)?),
                Ref::Import { dll, name } => Target::Fixed(
                    Class::Import,
                    stock.import_slot(dll, name).ok_or_else(|| format!("the DLL doesn't import {dll}!{name}"))?,
                ),
                Ref::Com => Target::Com,
                Ref::ComTail => Target::ComTail,
            };
            Ok((f.at, f.field, t))
        })
        .collect::<Result<_, String>>()?;
    let len = p.consts.len() as u32;
    if let Some(&(at, to)) =
        p.const_fixups.iter().find(|&&(at, to)| at.checked_add(4).is_none_or(|end| end > len) || to > len)
    {
        return Err(format!("the address at constant +{at:#x} (to +{to:#x}) is outside its {len} bytes"));
    }
    Ok(Placed { p, site, refs })
}

/// The patched image and its manifest. The relocation table goes in the code section, before the data section whose
/// constants it covers, so it is laid out until its length settles (once, unless it crosses a page).
fn build(stock: &Pe, exe: ExeId, placed: &[Placed]) -> Result<(Vec<u8>, Manifest), String> {
    let mut reloc_len = 0;
    for _ in 0..4 {
        match lay_out(stock, exe, placed, reloc_len)? {
            Ok(built) => return Ok(built),
            Err(len) => reloc_len = len,
        }
    }
    Err("internal: the relocation table's length doesn't settle".into())
}

/// [`build`] with the relocation table `reloc_len` bytes long; `Err` with its real length if that's wrong.
fn lay_out(
    stock: &Pe,
    exe: ExeId,
    placed: &[Placed],
    reloc_len: u32,
) -> Result<Result<(Vec<u8>, Manifest), u32>, String> {
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
            entry: 0,
            body_len: s.p.body.len() as u32,
            consts: 0,
            consts_len: s.p.consts.len() as u32,
            fixups: s
                .refs
                .iter()
                .map(|&(_, field, _)| (field, Class::Body))
                .chain(s.p.const_fixups.iter().map(|_| (Field::Abs32, Class::Const)))
                .map(|(field, class)| Fixup { field_rva: 0, field, class, target: 0 })
                .collect(),
        })
        .collect();
    let mut m = Manifest { exe, data_rva: 0, data_len: 0, com: 0, com_cache: 0, entries };
    // The manifest's length depends only on the counts and strings, which are final.
    let mut off = align(m.encode().len() as u32, CODE_ALIGN);
    for (e, s) in m.entries.iter_mut().zip(placed) {
        e.gate = code_rva + off;
        e.orig = e.gate + gate::LEN;
        e.body = code_rva + align(e.orig - code_rva + e.displace + 5, CODE_ALIGN);
        e.entry = e.body + s.p.entry;
        off = align(e.body - code_rva + e.body_len, CODE_ALIGN);
        for (f, r) in e.fixups.iter_mut().zip(&s.refs) {
            f.field_rva = e.body + r.0;
        }
    }
    if placed.iter().flat_map(|s| &s.refs).any(|r| matches!(r.2, Target::Com | Target::ComTail)) {
        m.com = code_rva + off;
        off = align(off + com::LEN, CODE_ALIGN);
    }
    let reloc_off = off;
    let code_len = reloc_off + reloc_len;

    // The data section: each patch's constants (with their addresses), then each patch's zero-filled data.
    let data_rva = align(code_rva + code_len, stock.section_align);
    let mut consts = Vec::new();
    for (e, s) in m.entries.iter_mut().zip(placed) {
        consts.resize(consts.len().next_multiple_of(4), 0);
        e.consts = data_rva + consts.len() as u32;
        consts.extend_from_slice(&s.p.consts);
        for (f, &(at, to)) in e.fixups[s.refs.len()..].iter_mut().zip(&s.p.const_fixups) {
            *f = Fixup { field_rva: e.consts + at, field: Field::Abs32, class: Class::Const, target: e.consts + to };
            put32(&mut consts, (f.field_rva - data_rva) as usize, stock.base.wrapping_add(f.target));
        }
    }
    let relocs = pe::reloc_table(&relocations(stock, &m.entries));
    if relocs.len() as u32 != reloc_len {
        return Ok(Err(relocs.len() as u32));
    }
    consts.resize(consts.len().next_multiple_of(DATA_ALIGN as usize), 0);
    m.data_rva = data_rva + consts.len() as u32;
    let mut data_offsets = Vec::new();
    for s in placed {
        data_offsets.push(m.data_len);
        m.data_len += align(s.p.data, DATA_ALIGN);
    }
    if m.com != 0 {
        m.com_cache = m.data_rva + m.data_len;
    }

    let mut code = vec![0xCC; code_len as usize];
    for ((e, s), data_off) in m.entries.iter_mut().zip(placed).zip(data_offsets) {
        for (f, (at, field, t)) in e.fixups.iter_mut().zip(&s.refs) {
            let (class, target) = match *t {
                Target::Fixed(class, rva) => (class, rva),
                Target::Body(o) => (Class::Body, e.body + o),
                Target::Data(o) => (Class::Data, m.data_rva + data_off + o),
                Target::Const(o) => (Class::Const, e.consts + o),
                Target::Original(o) => (Class::Original, e.orig + o),
                Target::Com => (Class::Com, m.com),
                Target::ComTail => (Class::ComTail, m.com + com::TAIL),
            };
            *f = Fixup { field_rva: e.body + at, field: *field, class, target };
        }
        let at = |rva: u32| (rva - code_rva) as usize;
        code[at(e.gate)..at(e.orig)].copy_from_slice(&gate::build(exe, e.gate, e.entry, e.orig));
        code[at(e.orig)..at(e.orig) + e.displace as usize + 5].copy_from_slice(&original(stock, e)?);
        let body = &mut code[at(e.body)..at(e.body) + e.body_len as usize];
        body.copy_from_slice(&s.p.body);
        for f in &e.fixups[..s.refs.len()] {
            let v = match f.field {
                Field::Rel32 => f.target.wrapping_sub(f.field_rva + 4),
                Field::Abs32 => stock.base.wrapping_add(f.target),
            };
            put32(body, (f.field_rva - e.body) as usize, v);
        }
    }
    if m.com != 0 {
        let at = (m.com - code_rva) as usize;
        code[at..at + com::LEN as usize].copy_from_slice(&com::build(m.com, m.com_cache));
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
    Ok(Ok((image, m)))
}

/// The data section's size in memory: the constants, the zero-filled data, and the Direct3D helper's cache (at least
/// 4 bytes).
pub(crate) fn data_vsize(m: &Manifest, data_rva: u32) -> u32 {
    let cache = if m.com != 0 { com::CACHE_LEN } else { 0 };
    (m.data_rva - data_rva + m.data_len + cache).max(4)
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
    /// thunk for each import and a call to each, a jump into `site2`, and a `je` to the end of
    /// `test eax, eax ; je ; xor eax, eax`),
    /// `.rdata` (imports `byondcore.dll!Foo` and `kernel32.dll!ExitProcess`, the export table, an exported `global`,
    /// and an exported `slot` in the IAT), and `.reloc`.
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
        put(&mut d, text(0x1080), &[0x85, 0xC0, 0x74, 0x02, 0x31, 0xC0, 0xC3]); // test ; je +2 ; xor ; ret
        put(&mut d, text(0x1090), &[0xFF, 0x25, 0xA8, 0x20, 0x00, 0x10]); // jmp [ExitProcess's slot]
        put(&mut d, text(0x10A0), &[0xE8, 0xEB, 0xFF, 0xFF, 0xFF]); // call that thunk
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

    /// dreamseeker.exe: the test DLL with `EXE`'s identity.
    fn exe() -> Vec<u8> {
        let mut exe = dll();
        let at = Pe::parse(&exe).unwrap().at;
        put32(&mut exe, at.file + 4, EXE.timestamp);
        put32(&mut exe, at.image_size(), EXE.image_size);
        exe
    }

    fn patch(site: &str, displace: u32, body: &[u8], fixups: &[(u32, Field, Ref)]) -> Patch {
        Patch {
            key: "p".into(),
            tier: crate::patch::Tier::Safe,
            name: "p".into(),
            about: "a test patch".into(),
            module: Module::Core,
            site: Place::Export(site.into()),
            displace,
            exe: Vec::new(),
            files: Vec::new(),
            body: body.to_vec(),
            entry: 0,
            fixups: fixups
                .iter()
                .map(|(at, field, target)| PatchFixup { at: *at, field: *field, target: target.clone() })
                .collect(),
            data: 4,
            consts: Vec::new(),
            const_fixups: Vec::new(),
        }
    }

    /// `lock inc dword ptr [data]`, then `jmp original`.
    fn counter() -> Patch {
        patch(
            "site",
            6,
            &[0xF0, 0xFF, 0x05, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0],
            &[(3, Field::Abs32, Ref::Data(0)), (8, Field::Rel32, Ref::Original(0))],
        )
    }

    fn applied(p: Patch) -> Vec<u8> {
        let a = apply(&dll(), Module::Core, &exe(), Some(EXE), &[p]).unwrap();
        a.image.unwrap_or_else(|| panic!("not applied: {:?}", a.lines))
    }

    fn skipped(p: Patch, why: &str) {
        let a = apply(&dll(), Module::Core, &exe(), Some(EXE), &[p]).unwrap();
        assert!(a.image.is_none() && a.lines[0].contains(why), "expected {why:?}, got {:?}", a.lines);
    }

    #[test]
    fn applies_and_audits() {
        let stock = dll();
        let image = applied(counter());
        let report = audit::audit(&image, &stock, Module::Core, EXE).unwrap();
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
        let image = applied(patch("relocsite", 5, &[0xE9, 0, 0, 0, 0], &[(1, Field::Rel32, Ref::Original(0))]));
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        let orig = m.entries[0].orig;
        // The site's address is relocated in the copy now, not in the site (which is a jmp).
        assert_eq!(pe.relocs, BTreeSet::from([orig + 1, 0x1042]));
        assert_eq!(pe.bytes(orig, 5).unwrap(), [0xA1, 0xF0, 0x21, 0x00, 0x10]);
        audit::audit(&image, &stock, Module::Core, EXE).unwrap();
        let mut inside = patch("site", 5, &[0xC3], &[]);
        inside.site = Place::Pattern("21 00 10 C3".into(), 0);
        skipped(inside, "isn't the start of an instruction");
    }

    #[test]
    fn fingerprint_sites() {
        // `relocsite` (mov eax, [global] ; ret): its relocated address is skipped, so a changed global still matches.
        let stock = dll();
        let bytes = Pe::parse(&stock).unwrap().bytes(0x1030, 6).unwrap().to_vec();
        let hash = dmfast_format::fingerprint::fnv1a(&bytes, &[(1, 4)]);
        let at = |hash| {
            let mut p = patch("site", 5, &[0xE9, 0, 0, 0, 0], &[(1, Field::Rel32, Ref::Original(0))]);
            p.site = Place::Fingerprint { prefix: "A1 ?? ?? ?? ?? C3".into(), len: 6, hash };
            p
        };
        let a = apply(&stock, Module::Core, &exe(), Some(EXE), &[at(hash)]).unwrap();
        assert_eq!(a.lines, ["p: applied at +0x1030"]);
        skipped(at(hash ^ 1), &format!("(+0x1030: {hash:#x})"));
    }

    #[test]
    fn exe_patterns() {
        // A patch applies only while dreamseeker.exe still has the code it was written against.
        let mut p = counter();
        p.exe = vec!["A1 ?? ?? ?? ?? C3".into()];
        applied(p.clone());
        p.exe.push("DE AD BE EF".into());
        skipped(p, "dreamseeker.exe: pattern \"DE AD BE EF\" not found");
    }

    #[test]
    fn exe_gates_check_the_patched_exe() {
        // Patching dreamseeker.exe itself: its gates must check for the patched file's identity, not the stock one's.
        let mut p = counter();
        p.module = Module::Seeker;
        let stock = exe();
        let image = apply(&stock, Module::Seeker, &stock, None, &[p]).unwrap().image.unwrap();
        let id = audit::exe_id(&image).unwrap();
        assert_ne!(id, EXE);
        let pe = Pe::parse(&image).unwrap();
        assert_eq!(Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap().exe, id);
        audit::audit(&image, &stock, Module::Seeker, id).unwrap();
    }

    #[test]
    fn site_branch_to_its_end_and_original_offset() {
        // jmp to the copy's `je` (offset 2), skipping its `test eax, eax`.
        let mut p = patch("site", 6, &[0xE9, 0, 0, 0, 0], &[(1, Field::Rel32, Ref::Original(2))]);
        p.site = Place::Pattern("85 C0 74 02 31 C0".into(), 0);
        let image = applied(p.clone());
        audit::audit(&image, &dll(), Module::Core, EXE).unwrap();
        let pe = Pe::parse(&image).unwrap();
        let e = &Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap().entries[0];
        // The copy keeps the `je`, which lands on the jump back.
        assert_eq!(pe.bytes(e.orig, 11).unwrap(), [&[0x85, 0xC0, 0x74, 0x02, 0x31, 0xC0][..], &e.jump_back()].concat());
        let mut mid = p.clone();
        mid.fixups[0].target = Ref::Original(3);
        skipped(mid, "branch fixup to something that isn't code");
        let mut out = p;
        out.displace = 7;
        skipped(out, "relative branch out of the site");
    }

    #[test]
    fn overlapping_sites() {
        assert_eq!(overlap(&[(0x1000, 6), (0x1010, 5), (0x1005, 5)]), Some((2, 0)));
        assert_eq!(overlap(&[(0x1000, 6), (0x1006, 5)]), None);
        let mut second = counter();
        second.name = "q".into();
        let a = apply(&dll(), Module::Core, &exe(), Some(EXE), &[counter(), second]).unwrap();
        assert!(a.lines.iter().any(|l| l.contains("q: skipped (its site overlaps p's)")), "{:?}", a.lines);
    }

    #[test]
    fn constants() {
        // push offset text ; pop eax ; jmp original
        let mut p = patch(
            "site",
            6,
            &[0x68, 0, 0, 0, 0, 0x58, 0xE9, 0, 0, 0, 0],
            &[(1, Field::Abs32, Ref::Const(0)), (7, Field::Rel32, Ref::Original(0))],
        );
        p.consts = b"hi\0".to_vec();
        let image = applied(p.clone());
        let report = audit::audit(&image, &dll(), Module::Core, EXE).unwrap();
        assert!(report.contains("constants: \"hi\""), "{report}");
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        assert_eq!(pe.bytes(m.entries[0].consts, 3).unwrap(), b"hi\0");
        assert!(!pe.is_code(m.entries[0].consts), "constants must not be executable");
        p.fixups[0].target = Ref::Const(3);
        skipped(p, "past its 3 bytes");
    }

    #[test]
    fn addresses_in_constants() {
        // push offset table ; pop eax ; jmp original, where the table holds the address of a string after it.
        let mut p = patch(
            "site",
            6,
            &[0x68, 0, 0, 0, 0, 0x58, 0xE9, 0, 0, 0, 0],
            &[(1, Field::Abs32, Ref::Const(0)), (7, Field::Rel32, Ref::Original(0))],
        );
        p.consts = b"\0\0\0\0hi\0".to_vec();
        p.const_fixups = vec![(0, 4)];
        let stock = dll();
        let image = applied(p.clone());
        audit::audit(&image, &stock, Module::Core, EXE).unwrap();
        let pe = Pe::parse(&image).unwrap();
        let e = &Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap().entries[0];
        assert_eq!(pe.bytes(e.consts, 4).unwrap(), (BASE + e.consts + 4).to_le_bytes());
        assert!(pe.relocs.contains(&e.consts), "the address is relocated");
        // An address in the constants that points out of them, or tampered with, fails.
        let mut out = p.clone();
        out.const_fixups = vec![(0, 8)];
        skipped(out, "outside its 7 bytes");
        let mut t = image.clone();
        let at = pe.offset(e.consts).unwrap();
        t[at] ^= 1;
        assert!(audit::audit(&t, &stock, Module::Core, EXE).unwrap_err().contains("doesn't hold its fixup's address"));
    }

    #[test]
    fn audit_catches_tampering() {
        let stock = dll();
        let image = applied(counter());
        let pe = Pe::parse(&image).unwrap();
        let code = *pe.section(&CODE_SECTION).unwrap();
        let m = Manifest::parse(code.bytes(&image)).unwrap();
        let fails = |img: &[u8], exe: ExeId, why: &str| {
            let err = audit::audit(img, &stock, Module::Core, exe).unwrap_err();
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
        fails(&t, EXE, "doesn't end in a jmp, ret, int3, or ud2");
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
            f.push((prefix.len() as u32 + 1, Field::Rel32, Ref::Original(0)));
            patch("site", 6, &body, &f)
        };
        let call = |place: Place| [(1, Field::Rel32, Ref::Code(place))];
        let slot = |dll: &str, name: &str| [(2, Field::Abs32, Ref::Import { dll: dll.into(), name: name.into() })];
        let e8 = [0xE8, 0, 0, 0, 0];
        let ff15 = [0xFF, 0x15, 0, 0, 0, 0];
        // Allowed: calling an exported function, an import slot of a BYOND DLL or its thunk, and reading BYOND's data.
        applied(jmp_orig(&e8, &call(Place::Export("helper".into()))));
        applied(jmp_orig(&ff15, &slot("byondcore.dll", "Foo")));
        applied(jmp_orig(&e8, &call(Place::Pattern("FF 25 A0 20 00 10".into(), 0))));
        applied(jmp_orig(&[0xA1, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Global(Place::Export("global".into())))]));
        // Not allowed.
        skipped(jmp_orig(&e8, &call(Place::Export("?Login@X@@QAEXXZ".into()))), "denied (login)");
        skipped(jmp_orig(&e8, &call(Place::Pattern("FF 25 A8 20 00 10".into(), 0))), "not a BYOND DLL");
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
        skipped(patch("site", 6, &[0x90], &[]), "doesn't end in a jmp, ret, int3, or ud2");
        skipped(jmp_orig(&[0x68, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Body(2))]), "not an instruction of the body");
        skipped(jmp_orig(&[0x68, 0, 0, 0, 0], &[(1, Field::Abs32, Ref::Com)]), "address that isn't");
    }

    /// `f`: `mov eax, offset f ; ret`, then `int3`s to 16; the entry: `call f ; jmp original` (`int3`s, then `ud2`
    /// when `ud2`).
    fn two_functions(ud2: bool) -> Patch {
        let mut body = vec![0xB8, 0, 0, 0, 0, 0xC3];
        body.resize(16, 0xCC);
        body.extend_from_slice(&[0xE8, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0]);
        if ud2 {
            body.extend_from_slice(&[0xCC, 0x0F, 0x0B]);
        }
        let mut p = patch(
            "site",
            6,
            &body,
            &[(1, Field::Abs32, Ref::Body(0)), (17, Field::Rel32, Ref::Body(0)), (22, Field::Rel32, Ref::Original(0))],
        );
        p.entry = 16;
        p
    }

    #[test]
    fn functions() {
        let stock = dll();
        let image = applied(two_functions(false));
        audit::audit(&image, &stock, Module::Core, EXE).unwrap();
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        let e = &m.entries[0];
        assert_eq!(e.entry, e.body + 16);
        assert_eq!(pe.bytes(e.gate, gate::LEN).unwrap(), gate::build(EXE, e.gate, e.entry, e.orig));
        // f's address, relocated; the call to it.
        assert_eq!(pe.bytes(e.body + 1, 4).unwrap(), (BASE + e.body).to_le_bytes());
        assert!(pe.relocs.contains(&(e.body + 1)));
        assert_eq!(pe.bytes(e.body + 17, 4).unwrap(), e.body.wrapping_sub(e.body + 21).to_le_bytes());
        assert_eq!((m.com, m.com_cache), (0, 0), "no helper without a patch using it");
        applied(two_functions(true));
        let mut p = two_functions(false);
        p.entry = 17;
        skipped(p, "the entry isn't an instruction of the body");
        let mut p = two_functions(false);
        p.fixups[0].target = Ref::Body(2);
        skipped(p, "takes the address +");
        let mut p = two_functions(false);
        p.entry = 26;
        skipped(p, "past its 26-byte body");
    }

    #[test]
    fn direct3d_helper() {
        let stock = dll();
        // push 3 ; push 2 ; push 1 ; push ecx ; push 17 ; call dmf_com ; jmp original
        let calls = patch(
            "site",
            6,
            &[0x6A, 3, 0x6A, 2, 0x6A, 1, 0x51, 0x6A, 17, 0xE8, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0],
            &[(10, Field::Rel32, Ref::Com), (15, Field::Rel32, Ref::Original(0))],
        );
        let image = applied(calls);
        let report = audit::audit(&image, &stock, Module::Core, EXE).unwrap();
        assert!(report.contains("calls Direct3D 9 methods (checked helper)"), "{report}");
        let pe = Pe::parse(&image).unwrap();
        let code = *pe.section(&CODE_SECTION).unwrap();
        let m = Manifest::parse(code.bytes(&image)).unwrap();
        assert!(m.com != 0 && m.com_cache == m.data_rva + m.data_len);
        assert_eq!(pe.bytes(m.com, com::LEN).unwrap(), com::build(m.com, m.com_cache));
        let e = &m.entries[0];
        assert_eq!(pe.bytes(e.body + 10, 4).unwrap(), m.com.wrapping_sub(e.body + 14).to_le_bytes());
        let data = pe.sections.last().unwrap();
        assert!(m.com_cache + com::CACHE_LEN <= data.rva + data.vsize, "the cache is in the data section");
        assert!(!pe.is_code(m.com_cache));
        let mut t = image.clone();
        t[code.raw_ptr as usize + (m.com - code.rva) as usize + 20] ^= 1;
        let err = audit::audit(&t, &stock, Module::Core, EXE).unwrap_err();
        assert!(err.contains("isn't part of any patch"), "{err}");
        // mov eax, 5 ; jmp dmf_com_tail
        let tail = patch("site", 6, &[0xB8, 5, 0, 0, 0, 0xE9, 0, 0, 0, 0], &[(6, Field::Rel32, Ref::ComTail)]);
        let image = applied(tail);
        let pe = Pe::parse(&image).unwrap();
        let m = Manifest::parse(pe.section(&CODE_SECTION).unwrap().bytes(&image)).unwrap();
        let e = &m.entries[0];
        assert_eq!(pe.bytes(e.body + 6, 4).unwrap(), (m.com + com::TAIL).wrapping_sub(e.body + 10).to_le_bytes());
    }
}
