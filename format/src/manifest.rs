//! The manifest at the start of a patched DLL's [`CODE_SECTION`]: which sites were rewritten, where their gates,
//! original instructions, and bodies are, and every address a body uses. The audit re-checks a patched DLL from it,
//! and dmfast.dll reads it to put BYOND's own bytes back at each site.
//!
//! All numbers are little-endian `u32`s; addresses are RVAs. Layout:
//! - header: [`MAGIC`], the gate's program identity (dreamseeker.exe's `TimeDateStamp` and `SizeOfImage`), the
//!   entry count, then the offsets (from the section start) of the entries, fixups, and strings, the fixup count,
//!   the patches' data area (RVA, length), and the Direct3D helper and its cache (RVAs, 0 without them);
//! - entries, [`ENTRY_LEN`] bytes each: site, displaced length, gate, original instructions, body, body length, first
//!   fixup, fixup count, name, and description (offsets into the strings), constants and their length, and the
//!   entry (where in the body the gate jumps to);
//! - fixups, [`FIXUP_LEN`] bytes each: the field's RVA, its [`Field`] and [`Class`] (one byte each, then two zero
//!   bytes), and the target's RVA (an import's: its IAT slot);
//! - strings, NUL-terminated UTF-8.

/// The section holding the manifest, gates, original instructions, bodies, and the new base relocation table
/// (read and execute).
pub const CODE_SECTION: [u8; 8] = *b".dmfsk\0\0";
/// The patches' zero-filled data (read and write).
pub const DATA_SECTION: [u8; 8] = *b".dmfskd\0";
const MAGIC: [u8; 8] = *b"DMFSEEK\x02";
const HEADER_LEN: usize = 52;
const ENTRY_LEN: usize = 52;
const FIXUP_LEN: usize = 12;

/// The program whose gates are open: dreamseeker.exe, by its PE header's `TimeDateStamp` and `SizeOfImage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExeId {
    pub timestamp: u32,
    pub image_size: u32,
}

/// A 4-byte field in a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A `call`, `jmp`, or `jcc` displacement.
    Rel32 = 0,
    /// An absolute address (relocated by the loader).
    Abs32 = 1,
}

/// What a field points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Inside the same body.
    Body = 0,
    /// The patches' data area.
    Data = 1,
    /// The instruction after the site (the displaced bytes are skipped).
    Resume = 2,
    /// The site's original instructions, which then continue after the site.
    Original = 3,
    /// Code in the same DLL.
    Code = 4,
    /// BYOND's data in the same DLL.
    Global = 5,
    /// An existing import slot of the DLL (`call dword ptr [slot]`).
    Import = 6,
    /// The patch's own read-only constants.
    Const = 7,
    /// The Direct3D helper's `dmf_com` (`com.rs`).
    Com = 8,
    /// The Direct3D helper's `dmf_com_tail`.
    ComTail = 9,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixup {
    pub field_rva: u32,
    pub field: Field,
    pub class: Class,
    pub target: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub about: String,
    /// The rewritten site: a `jmp` to the gate, then `int3`s up to `displace` bytes.
    pub site: u32,
    pub displace: u32,
    pub gate: u32,
    /// A copy of the site's `displace` original bytes, then a `jmp` back to `site + displace`.
    pub orig: u32,
    pub body: u32,
    /// Where the gate jumps to, in the body.
    pub entry: u32,
    pub body_len: u32,
    /// The patch's read-only constants (data, never run), after its body.
    pub consts: u32,
    pub consts_len: u32,
    pub fixups: Vec<Fixup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub exe: ExeId,
    pub data_rva: u32,
    pub data_len: u32,
    /// The Direct3D helper (`com.rs`) and its cache; 0 when no patch uses it.
    pub com: u32,
    pub com_cache: u32,
    pub entries: Vec<Entry>,
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn str_at(d: &[u8], o: usize) -> Option<String> {
    let s = d.get(o..)?;
    let end = s.iter().position(|&b| b == 0)?;
    String::from_utf8(s[..end].to_vec()).ok()
}

impl Field {
    fn from_u8(v: u8) -> Option<Field> {
        Some(match v {
            0 => Field::Rel32,
            1 => Field::Abs32,
            _ => return None,
        })
    }
}

impl Class {
    fn from_u8(v: u8) -> Option<Class> {
        Some(match v {
            0 => Class::Body,
            1 => Class::Data,
            2 => Class::Resume,
            3 => Class::Original,
            4 => Class::Code,
            5 => Class::Global,
            6 => Class::Import,
            7 => Class::Const,
            8 => Class::Com,
            9 => Class::ComTail,
            _ => return None,
        })
    }
}

/// A `jmp rel32`'s length.
pub const JMP_LEN: u32 = 5;

/// `jmp to`, as an instruction at `at` (RVAs: the same at any load address).
fn jmp(at: u32, to: u32) -> [u8; JMP_LEN as usize] {
    let rel = to.wrapping_sub(at + JMP_LEN).to_le_bytes();
    [0xE9, rel[0], rel[1], rel[2], rel[3]]
}

impl Entry {
    /// What its site holds in a patched DLL: a `jmp` to its gate, then `int3`s up to `displace` bytes.
    pub fn site_bytes(&self) -> Vec<u8> {
        let mut b = jmp(self.site, self.gate).to_vec();
        b.resize(self.displace.max(JMP_LEN) as usize, 0xCC);
        b
    }

    /// What follows the copy of its original instructions: a `jmp` back to the instruction after the site.
    pub fn jump_back(&self) -> [u8; JMP_LEN as usize] {
        jmp(self.orig + self.displace, self.site + self.displace)
    }
}

impl Manifest {
    /// Reads the manifest at the start of `section` (the [`CODE_SECTION`]'s bytes); `None` if it isn't one.
    pub fn parse(section: &[u8]) -> Option<Manifest> {
        if section.get(..8)? != MAGIC {
            return None;
        }
        let h = |i: usize| u32_at(section, 8 + 4 * i);
        let (timestamp, image_size, count) = (h(0)?, h(1)?, h(2)? as usize);
        let (entries, fixups, strings, fixup_count) = (h(3)? as usize, h(4)? as usize, h(5)? as usize, h(6)? as usize);
        let (data_rva, data_len, com, com_cache) = (h(7)?, h(8)?, h(9)?, h(10)?);
        let fixup = |i: usize| -> Option<Fixup> {
            let o = fixups.checked_add(i.checked_mul(FIXUP_LEN)?)?;
            let (field, class, pad) = (*section.get(o + 4)?, *section.get(o + 5)?, u32_at(section, o + 4)? >> 16);
            (pad == 0).then_some(())?;
            Some(Fixup {
                field_rva: u32_at(section, o)?,
                field: Field::from_u8(field)?,
                class: Class::from_u8(class)?,
                target: u32_at(section, o + 8)?,
            })
        };
        let mut out = Vec::with_capacity(count.min(4096));
        for i in 0..count {
            let o = entries.checked_add(i.checked_mul(ENTRY_LEN)?)?;
            let e = |k: usize| u32_at(section, o + 4 * k);
            let (first, n) = (e(6)? as usize, e(7)? as usize);
            if first.checked_add(n)? > fixup_count {
                return None;
            }
            out.push(Entry {
                site: e(0)?,
                displace: e(1)?,
                gate: e(2)?,
                orig: e(3)?,
                body: e(4)?,
                entry: e(12)?,
                body_len: e(5)?,
                consts: e(10)?,
                consts_len: e(11)?,
                fixups: (first..first + n).map(fixup).collect::<Option<_>>()?,
                name: str_at(section, strings.checked_add(e(8)? as usize)?)?,
                about: str_at(section, strings.checked_add(e(9)? as usize)?)?,
            });
        }
        Some(Manifest { exe: ExeId { timestamp, image_size }, data_rva, data_len, com, com_cache, entries: out })
    }

    /// The manifest's bytes, for the start of the [`CODE_SECTION`].
    pub fn encode(&self) -> Vec<u8> {
        let count = self.entries.len();
        let fixup_count: usize = self.entries.iter().map(|e| e.fixups.len()).sum();
        let entries = HEADER_LEN;
        let fixups = entries + ENTRY_LEN * count;
        let strings = fixups + FIXUP_LEN * fixup_count;
        let mut d = Vec::new();
        d.extend_from_slice(&MAGIC);
        for v in [self.exe.timestamp, self.exe.image_size, count as u32, entries as u32, fixups as u32, strings as u32]
        {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for v in [fixup_count as u32, self.data_rva, self.data_len, self.com, self.com_cache] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let mut text = Vec::new();
        let mut put_str = |s: &str| {
            let at = text.len() as u32;
            text.extend_from_slice(s.as_bytes());
            text.push(0);
            at
        };
        let mut first = 0;
        for e in &self.entries {
            let (name, about) = (put_str(&e.name), put_str(&e.about));
            for v in [
                e.site,
                e.displace,
                e.gate,
                e.orig,
                e.body,
                e.body_len,
                first,
                e.fixups.len() as u32,
                name,
                about,
                e.consts,
                e.consts_len,
                e.entry,
            ] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            first += e.fixups.len() as u32;
        }
        for f in self.entries.iter().flat_map(|e| &e.fixups) {
            d.extend_from_slice(&f.field_rva.to_le_bytes());
            d.extend_from_slice(&[f.field as u8, f.class as u8, 0, 0]);
            d.extend_from_slice(&f.target.to_le_bytes());
        }
        d.extend_from_slice(&text);
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = Manifest {
            exe: ExeId { timestamp: 0x1234_5678, image_size: 0xD_0000 },
            data_rva: 0x50_0000,
            data_len: 8,
            com: 0x40_0300,
            com_cache: 0x50_0008,
            entries: vec![
                Entry {
                    name: "one".into(),
                    about: "first".into(),
                    site: 0x1000,
                    displace: 6,
                    gate: 0x40_0100,
                    orig: 0x40_0140,
                    body: 0x40_0150,
                    entry: 0x40_0158,
                    body_len: 20,
                    consts: 0x40_0170,
                    consts_len: 3,
                    fixups: vec![Fixup {
                        field_rva: 0x40_0151,
                        field: Field::Abs32,
                        class: Class::Data,
                        target: 0x50_0000,
                    }],
                },
                Entry {
                    name: "two".into(),
                    about: String::new(),
                    site: 0x2000,
                    displace: 5,
                    gate: 0x40_0200,
                    orig: 0x40_0240,
                    body: 0x40_0250,
                    entry: 0x40_0250,
                    body_len: 5,
                    consts: 0,
                    consts_len: 0,
                    fixups: vec![Fixup {
                        field_rva: 0x40_0251,
                        field: Field::Rel32,
                        class: Class::ComTail,
                        target: 0x40_0304,
                    }],
                },
            ],
        };
        let bytes = m.encode();
        assert_eq!(Manifest::parse(&bytes), Some(m));
        assert_eq!(Manifest::parse(&bytes[..bytes.len() - 1]), None, "a cut string is not a manifest");
        assert_eq!(Manifest::parse(b"DMFSEEK\x01"), None, "an older manifest");
    }
}
