//! A PE32 image as stored on disk: its sections, exports, imports, base relocations, and a linear sweep of its code;
//! and appending sections to it.

use std::collections::{BTreeSet, HashMap};

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
/// Code, execute, read.
pub const CODE_FLAGS: u32 = 0x6000_0020;
/// Initialised data, read, write.
pub const DATA_FLAGS: u32 = 0xC000_0040;
const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_HIGHLOW: u16 = 3;
const DIR_EXPORT: usize = 0;
pub const DIR_IMPORT: usize = 1;
const DIR_SECURITY: usize = 4;
pub const DIR_BASERELOC: usize = 5;
pub const DIR_IAT: usize = 12;
const DIR_DELAY_IMPORT: usize = 13;

pub(crate) fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

pub fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn align(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub rva: u32,
    pub vsize: u32,
    pub raw_size: u32,
    pub raw_ptr: u32,
    pub flags: u32,
}

impl Section {
    pub fn exec(&self) -> bool {
        self.flags & IMAGE_SCN_MEM_EXECUTE != 0
    }

    /// The RVAs it occupies in memory.
    pub fn span(&self) -> std::ops::Range<u32> {
        self.rva..self.rva + self.vsize.max(self.raw_size)
    }

    /// Its bytes in the file (no more than its virtual size).
    pub fn bytes<'a>(&self, d: &'a [u8]) -> &'a [u8] {
        let len = if self.vsize == 0 { self.raw_size } else { self.raw_size.min(self.vsize) };
        d.get(self.raw_ptr as usize..(self.raw_ptr + len) as usize).unwrap_or(&[])
    }
}

/// Where the headers' fields are in the file.
#[derive(Clone, Copy)]
pub struct Headers {
    /// The file header (after `PE\0\0`).
    pub file: usize,
    /// The optional header.
    pub opt: usize,
    /// The section table.
    pub table: usize,
}

impl Headers {
    pub fn sections(self) -> usize {
        self.file + 2
    }
    pub fn image_size(self) -> usize {
        self.opt + 56
    }
    pub fn checksum(self) -> usize {
        self.opt + 64
    }
    pub fn dir(self, i: usize) -> usize {
        self.opt + 96 + 8 * i
    }
}

pub struct Pe<'a> {
    pub data: &'a [u8],
    pub at: Headers,
    pub base: u32,
    pub section_align: u32,
    pub file_align: u32,
    pub headers_size: u32,
    pub sections: Vec<Section>,
    /// Exports by RVA (sorted), and by name.
    exports: Vec<(u32, String)>,
    by_name: HashMap<String, u32>,
    /// `dll` (lower case), name, and IAT slot RVA of each imported name, delay-loaded ones included.
    imports: Vec<(String, String, u32)>,
    /// The fields the loader relocates.
    pub relocs: BTreeSet<u32>,
}

fn c_str(d: &[u8], o: usize) -> Option<String> {
    let s = d.get(o..)?;
    Some(String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0)?]).into_owned())
}

impl<'a> Pe<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Pe<'a>, String> {
        let bad = || "not a PE32 image".to_string();
        let nt = u32_at(data, 0x3C).ok_or_else(bad)? as usize;
        if data.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err(bad());
        }
        let file = nt + 4;
        let opt = file + 20;
        if u16_at(data, opt) != Some(0x10B) {
            return Err(bad());
        }
        let at = Headers { file, opt, table: opt + u16_at(data, file + 16).ok_or_else(bad)? as usize };
        let n = u16_at(data, at.sections()).ok_or_else(bad)? as usize;
        let field = |o: usize| u32_at(data, o).ok_or_else(bad);
        let sections = (0..n)
            .map(|i| {
                let s = at.table + 40 * i;
                Ok(Section {
                    name: data.get(s..s + 8).ok_or_else(bad)?.try_into().unwrap(),
                    vsize: field(s + 8)?,
                    rva: field(s + 12)?,
                    raw_size: field(s + 16)?,
                    raw_ptr: field(s + 20)?,
                    flags: field(s + 36)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut pe = Pe {
            data,
            at,
            base: field(opt + 28)?,
            section_align: field(opt + 32)?,
            file_align: field(opt + 36)?,
            headers_size: field(opt + 60)?,
            sections,
            exports: Vec::new(),
            by_name: HashMap::new(),
            imports: Vec::new(),
            relocs: BTreeSet::new(),
        };
        pe.read_exports().ok_or("bad export table")?;
        pe.read_imports().ok_or("bad import table")?;
        pe.read_delay_imports().ok_or("bad delay import table")?;
        pe.read_relocs()?;
        Ok(pe)
    }

    /// A data directory's RVA and size.
    pub fn dir(&self, i: usize) -> (u32, u32) {
        let o = self.at.dir(i);
        (u32_at(self.data, o).unwrap_or(0), u32_at(self.data, o + 4).unwrap_or(0))
    }

    /// Whether `rva` is in data directory `i`.
    pub fn in_dir(&self, i: usize, rva: u32) -> bool {
        let (at, size) = self.dir(i);
        (at..at.saturating_add(size)).contains(&rva)
    }

    pub fn offset(&self, rva: u32) -> Option<usize> {
        if rva < self.headers_size {
            return Some(rva as usize);
        }
        let s = self.sections.iter().find(|s| (s.rva..s.rva + s.raw_size).contains(&rva))?;
        Some((s.raw_ptr + rva - s.rva) as usize)
    }

    pub fn bytes(&self, rva: u32, len: u32) -> Option<&'a [u8]> {
        let o = self.offset(rva)?;
        let end = self.offset(rva.checked_add(len.checked_sub(1)?)?)? + 1;
        (end == o + len as usize).then(|| &self.data[o..end])
    }

    fn u32(&self, rva: u32) -> Option<u32> {
        u32_at(self.data, self.offset(rva)?)
    }

    fn str(&self, rva: u32) -> Option<String> {
        c_str(self.data, self.offset(rva)?)
    }

    pub fn section(&self, name: &[u8; 8]) -> Option<&Section> {
        self.sections.iter().find(|s| &s.name == name)
    }

    pub fn section_of(&self, rva: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.span().contains(&rva))
    }

    /// In an executable section.
    pub fn is_code(&self, rva: u32) -> bool {
        self.section_of(rva).is_some_and(Section::exec)
    }

    fn read_exports(&mut self) -> Option<()> {
        let (dir, size) = self.dir(DIR_EXPORT);
        if dir == 0 {
            return Some(());
        }
        let (n, funcs, names, ords) =
            (self.u32(dir + 24)?, self.u32(dir + 28)?, self.u32(dir + 32)?, self.u32(dir + 36)?);
        for i in 0..n {
            let name = self.str(self.u32(names + 4 * i)?)?;
            let ord = u16_at(self.data, self.offset(ords + 2 * i)?)? as u32;
            let rva = self.u32(funcs + 4 * ord)?;
            // A forwarder names another DLL's export.
            if !(dir..dir + size).contains(&rva) {
                self.by_name.insert(name.clone(), rva);
                self.exports.push((rva, name));
            }
        }
        self.exports.sort();
        Some(())
    }

    fn read_imports(&mut self) -> Option<()> {
        let (mut desc, _) = self.dir(DIR_IMPORT);
        if desc == 0 {
            return Some(());
        }
        loop {
            let (ilt, name, iat) = (self.u32(desc)?, self.u32(desc + 12)?, self.u32(desc + 16)?);
            if name == 0 {
                return Some(());
            }
            let dll = self.str(name)?.to_ascii_lowercase();
            let list = if ilt != 0 { ilt } else { iat };
            for i in 0.. {
                let thunk = self.u32(list + 4 * i)?;
                if thunk == 0 {
                    break;
                }
                // Imports by ordinal have no name to allow or deny.
                if thunk & 0x8000_0000 == 0 {
                    self.imports.push((dll.clone(), self.str(thunk + 2)?, iat + 4 * i));
                }
            }
            desc += 20;
        }
    }

    /// Delay-loaded imports, whose slots a `jmp [slot]` stub calls through.
    fn read_delay_imports(&mut self) -> Option<()> {
        let (mut desc, _) = self.dir(DIR_DELAY_IMPORT);
        if desc == 0 {
            return Some(());
        }
        loop {
            // Attributes, then RVAs (attribute bit 0) of the name, module handle, IAT, and name table.
            let (name, iat, int) = (self.u32(desc + 4)?, self.u32(desc + 12)?, self.u32(desc + 16)?);
            if name == 0 {
                return Some(());
            }
            let dll = self.str(name)?.to_ascii_lowercase();
            for i in 0.. {
                let thunk = self.u32(int + 4 * i)?;
                if thunk == 0 {
                    break;
                }
                if thunk & 0x8000_0000 == 0 {
                    self.imports.push((dll.clone(), self.str(thunk + 2)?, iat + 4 * i));
                }
            }
            desc += 32;
        }
    }

    fn read_relocs(&mut self) -> Result<(), String> {
        let (dir, size) = self.dir(DIR_BASERELOC);
        let bad = || "bad base relocation table".to_string();
        let mut at = dir;
        while at < dir + size {
            let (page, len) = (self.u32(at).ok_or_else(bad)?, self.u32(at + 4).ok_or_else(bad)?);
            if len < 8 {
                return Err(bad());
            }
            let entries = self.bytes(at + 8, len - 8).ok_or_else(bad)?;
            for e in entries.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)) {
                match e >> 12 {
                    IMAGE_REL_BASED_HIGHLOW => {
                        self.relocs.insert(page + u32::from(e & 0xFFF));
                    }
                    IMAGE_REL_BASED_ABSOLUTE => {}
                    t => return Err(format!("base relocation type {t} (a PE32 DLL has only HIGHLOW)")),
                }
            }
            at += len;
        }
        Ok(())
    }

    pub fn export(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    /// The names exported at `rva`.
    pub fn exports_at(&self, rva: u32) -> impl Iterator<Item = &str> {
        let from = self.exports.partition_point(|e| e.0 < rva);
        self.exports[from..].iter().take_while(move |e| e.0 == rva).map(|e| e.1.as_str())
    }

    /// The nearest export at or before `rva`, and its RVA.
    pub fn export_before(&self, rva: u32) -> Option<(&str, u32)> {
        let i = self.exports.partition_point(|e| e.0 <= rva);
        self.exports[..i].last().map(|e| (e.1.as_str(), e.0))
    }

    /// The IAT slot importing `dll!name` (`dll` case-insensitive).
    pub fn import_slot(&self, dll: &str, name: &str) -> Option<u32> {
        self.imports.iter().find(|i| i.0.eq_ignore_ascii_case(dll) && i.1 == name).map(|i| i.2)
    }

    /// The `dll` (lower case) and name an IAT slot imports.
    pub fn import_at(&self, slot: u32) -> Option<(&str, &str)> {
        self.imports.iter().find(|i| i.2 == slot).map(|i| (i.0.as_str(), i.1.as_str()))
    }

    /// Every match of an IDA-style pattern in the executable sections.
    pub fn find_all(&self, ida: &str) -> Result<Vec<u32>, String> {
        let pat = ida
            .split_whitespace()
            .map(|t| if t == "??" || t == "?" { Ok(None) } else { u8::from_str_radix(t, 16).map(Some) })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("bad pattern {ida:?}"))?;
        if pat.is_empty() {
            return Err("empty pattern".into());
        }
        let mut hits = Vec::new();
        for s in self.sections.iter().filter(|s| s.exec()) {
            let code = s.bytes(self.data);
            for (i, w) in code.windows(pat.len()).enumerate() {
                if pat.iter().zip(w).all(|(p, b)| p.is_none_or(|p| p == *b)) {
                    hits.push(s.rva + i as u32);
                }
            }
        }
        Ok(hits)
    }

    /// The single match of an IDA-style pattern in the executable sections.
    pub fn find_pattern(&self, ida: &str) -> Result<u32, String> {
        match self.find_all(ida)?[..] {
            [one] => Ok(one),
            [] => Err(format!("pattern {ida:?} not found")),
            ref hits => Err(format!("pattern {ida:?} found {} times", hits.len())),
        }
    }

    /// The fingerprint hash of the `len` bytes at `rva` (see [`Place::Fingerprint`](crate::patch::Place)): the
    /// relocated addresses in them and the targets of their `call`, `jmp`, and `jcc` instructions that leave them are
    /// skipped, which a build's layout changes without changing the function. `None` outside the code.
    pub fn fingerprint(&self, rva: u32, len: u32) -> Option<u64> {
        let bytes = self.bytes(rva, len).filter(|_| self.is_code(rva) && len <= u32::from(u16::MAX))?;
        let end = rva + len;
        let mut holes: Vec<(u16, u16)> =
            self.relocs.range(rva.saturating_sub(3)..end).map(|&r| hole(rva, r, end)).collect();
        for ins in Decoder::with_ip(32, bytes, u64::from(rva), DecoderOptions::NONE) {
            let to = ins.near_branch_target() as u32;
            if ins.op0_kind() == OpKind::NearBranch32 && !(rva..end).contains(&to) {
                holes.push(hole(rva, ins.ip32() + ins.len() as u32 - 4, end));
            }
        }
        holes.sort();
        Some(dmfast_format::fingerprint::fnv1a(bytes, &holes))
    }

    /// The RVA a section added after the last one starts at.
    pub fn next_rva(&self) -> u32 {
        self.sections.iter().map(|s| align(s.span().end, self.section_align)).max().unwrap_or(0)
    }

    /// A linear sweep of the executable sections: where instructions start, where near branches go, and which of
    /// those are calls. Data inside code (jump tables) briefly misleads it, so it answers "probably": good enough to
    /// refuse a site something branches into, or a call that isn't to a function.
    pub fn sweep(&self) -> Sweep {
        let words = (self.next_rva() as usize).div_ceil(64);
        let mut sw = Sweep { starts: vec![0; words], targets: vec![0; words], calls: vec![0; words] };
        let mut ins = Instruction::default();
        for s in self.sections.iter().filter(|s| s.exec()) {
            let mut dec = Decoder::with_ip(32, s.bytes(self.data), u64::from(self.base + s.rva), DecoderOptions::NONE);
            while dec.can_decode() {
                dec.decode_out(&mut ins);
                if ins.is_invalid() {
                    continue;
                }
                set(&mut sw.starts, ins.ip32() - self.base);
                if matches!(ins.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16) {
                    let to = (ins.near_branch_target() as u32).wrapping_sub(self.base);
                    set(&mut sw.targets, to);
                    if ins.is_call_near() {
                        set(&mut sw.calls, to);
                    }
                }
            }
        }
        sw
    }
}

/// The hole for the 4-byte field at `at` in a function at `start` that ends at `end`.
fn hole(start: u32, at: u32, end: u32) -> (u16, u16) {
    let from = at.max(start);
    ((from - start) as u16, ((at + 4).min(end) - from) as u16)
}

fn set(bits: &mut [u64], i: u32) {
    if let Some(w) = bits.get_mut(i as usize / 64) {
        *w |= 1 << (i % 64);
    }
}

fn get(bits: &[u64], i: u32) -> bool {
    bits.get(i as usize / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)
}

pub struct Sweep {
    starts: Vec<u64>,
    targets: Vec<u64>,
    calls: Vec<u64>,
}

impl Sweep {
    pub fn starts_at(&self, rva: u32) -> bool {
        get(&self.starts, rva)
    }

    /// Whether some code calls `rva` (it starts a function).
    pub fn called(&self, rva: u32) -> bool {
        get(&self.calls, rva)
    }

    /// Whether a near branch goes to any of `rvas`.
    pub fn branched_into(&self, rvas: std::ops::Range<u32>) -> Option<u32> {
        rvas.into_iter().find(|&r| get(&self.targets, r))
    }
}

/// A base relocation table holding `fields` (HIGHLOW entries, one block per page, each padded to 4 bytes).
pub fn reloc_table(fields: &BTreeSet<u32>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut fields = fields.iter().peekable();
    while let Some(&first) = fields.peek() {
        let page = first & !0xFFF;
        let mut entries = Vec::new();
        while let Some(&&f) = fields.peek() {
            if f & !0xFFF != page {
                break;
            }
            entries.push((IMAGE_REL_BASED_HIGHLOW << 12) | (f & 0xFFF) as u16);
            fields.next();
        }
        if entries.len() % 2 == 1 {
            entries.push(IMAGE_REL_BASED_ABSOLUTE << 12);
        }
        out.extend_from_slice(&page.to_le_bytes());
        out.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
        for e in entries {
            out.extend_from_slice(&e.to_le_bytes());
        }
    }
    out
}

/// A section to append: its name, characteristics, bytes in the file, and size in memory (zero-filled past the bytes).
pub struct NewSection<'a> {
    pub name: [u8; 8],
    pub flags: u32,
    pub data: &'a [u8],
    pub vsize: u32,
}

/// `stock` with `sections` appended in order, the first at [`Pe::next_rva`] and each next one at the section alignment
/// after the last, and data directories `dirs` (index, RVA, size) set. The headers change only in the section count,
/// the image size, the checksum (0: not checked for user-mode images), those directories, and the new section headers.
pub fn append(stock: &Pe, sections: &[NewSection], dirs: &[(usize, u32, u32)]) -> Result<Vec<u8>, String> {
    let n = stock.sections.len();
    let first_raw = stock.sections.iter().map(|s| s.raw_ptr).filter(|&p| p != 0).min().unwrap_or(stock.headers_size);
    let new = stock.at.table + 40 * n;
    let headers = new..new + 40 * sections.len();
    if headers.end > stock.headers_size.min(first_raw) as usize || stock.data[headers].iter().any(|&b| b != 0) {
        return Err(format!("no room for {} more section headers", sections.len()));
    }
    let end = stock.sections.iter().map(|s| s.raw_ptr + s.raw_size).max().unwrap_or(0) as usize;
    if stock.data.len() != end || stock.dir(DIR_SECURITY) != (0, 0) {
        return Err("the image has data after its sections (a signature or overlay)".into());
    }
    let mut d = stock.data.to_vec();
    let (mut rva, mut image_end) = (stock.next_rva(), stock.next_rva());
    for (k, s) in sections.iter().enumerate() {
        let vsize = s.vsize.max(s.data.len() as u32);
        let raw_size = align(s.data.len() as u32, stock.file_align);
        let raw_ptr = if s.data.is_empty() { 0 } else { align(d.len() as u32, stock.file_align) };
        if raw_ptr != 0 {
            d.resize(raw_ptr as usize, 0);
            d.extend_from_slice(s.data);
            d.resize((raw_ptr + raw_size) as usize, 0);
        }
        let h = new + 40 * k;
        d[h..h + 8].copy_from_slice(&s.name);
        for (o, v) in [(8, vsize), (12, rva), (16, raw_size), (20, raw_ptr), (36, s.flags)] {
            put32(&mut d, h + o, v);
        }
        image_end = rva + vsize;
        rva = align(image_end, stock.section_align);
    }
    d[stock.at.sections()..stock.at.sections() + 2].copy_from_slice(&((n + sections.len()) as u16).to_le_bytes());
    put32(&mut d, stock.at.image_size(), align(image_end, stock.section_align));
    put32(&mut d, stock.at.checksum(), 0);
    for &(i, at, size) in dirs {
        put32(&mut d, stock.at.dir(i), at);
        put32(&mut d, stock.at.dir(i) + 4, size);
    }
    Ok(d)
}
