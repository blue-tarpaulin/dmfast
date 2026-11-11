//! Private patches as their author writes them, turned into [`Patch`]es: a `no_std` Rust file compiled to an i686
//! COFF object ([`compile`]), and a spec that says where each patch goes and what each outside symbol its code uses
//! is. The spec has one directive per line (blank lines and lines starting with `#` are skipped); a `patch` line
//! starts a patch and the lines after it describe that patch:
//! ```text
//! patch <name> <byondcore.dll|byondwin.dll>
//! tier <safe|extended|breaking>
//! about <text>
//! site export <decorated name>
//! site pattern <offset> <IDA bytes...>
//! site fingerprint <length> <hash> <IDA bytes...>
//! displace <n>
//! body <symbol>
//! data <bytes>
//! ref <symbol> code|global export <name>
//! ref <symbol> code|global pattern <offset> <IDA bytes...>
//! ref <symbol> import <dll> <name>
//! ```
//! `about`, `tier`, `site`, `displace`, and `body` are required; `data` defaults to 0; numbers are decimal or `0x`
//! hex. `site` may repeat: the same code at several sites (patches `<name>`, `<name>-2`, ...). A fingerprint names a
//! whole function without its bytes ([`Place::Fingerprint`]): the bytes are only how it starts, and an install that
//! finds no match says what each function starting that way hashes to. `dmf_resume`, `dmf_original`, `dmf_data`,
//! `dmf_com`, and `dmf_com_tail` need no `ref`; `jmp dmf_original + n` runs the original instructions from offset
//! `n`. Names are matched without the `_` (and a stdcall `@<bytes>`) i686 Windows puts on C names.
//!
//! A patch's code is every function of the object, in order, each aligned as its section says with `int3`s between;
//! the gate enters it at the `body` function. Its relocations become fixups (`DIR32`: `Abs32`, `REL32`: `Rel32`), the
//! in-place value an addend: to the code (a call, a label, a function's address), to a zero-initialised `static`
//! (which becomes the start of the patch's data, `data` after them, where `dmf_data` points), to a read-only
//! `static` (whose bytes join the patch's constants), to a built-in, or to a `ref`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use object::read::coff::CoffFile;
use object::{
    Architecture, Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionIndex, SectionKind,
    SymbolIndex, SymbolKind, pe,
};

use crate::patch::{Dll, Fixup, Patch, Place, Ref, Tier};
use dmfast_format::manifest::Field;

macro_rules! bail {
    ($($t:tt)*) => {
        return Err(format!($($t)*))
    };
}

const RESUME: &str = "dmf_resume";
const ORIGINAL: &str = "dmf_original";
const DATA: &str = "dmf_data";
const COM: &str = "dmf_com";
const COM_TAIL: &str = "dmf_com_tail";
/// The prelude's trap, where core's panic paths go.
const TRAP: &str = "dmf_trap";
/// The most alignment the code or the data can give: each body and each patch's data start on this boundary.
const MAX_ALIGN: u64 = 16;

/// Turns the compiled patches (`obj`, an i686 COFF object) and their `spec` into patches, all with `key` (their
/// folder). An error about a patch names its spec line.
pub fn from_object(obj: &[u8], spec: &str, key: &str) -> Result<Vec<Patch>, String> {
    let drafts = parse_spec(spec, key)?;
    let file = CoffFile::<&[u8]>::parse(obj).map_err(|e| format!("the object isn't COFF: {e}"))?;
    if file.architecture() != Architecture::I386 {
        bail!("the object is for {:?}, not i686", file.architecture());
    }
    let mut patches = Vec::new();
    for (mut patch, source) in drafts {
        read_code(&file, &source, &mut patch)
            .map_err(|e| format!("line {}: patch {}: {e}", source.line, patch.name))?;
        for (i, site) in source.sites.into_iter().enumerate() {
            let name = if i == 0 { patch.name.clone() } else { format!("{}-{}", patch.name, i + 1) };
            patches.push(Patch { name, site, ..patch.clone() });
        }
    }
    Ok(patches)
}

/// Where a patch's code comes from: its `body` line and symbol, and its `ref`s by symbol; and its sites.
struct Source {
    line: usize,
    symbol: String,
    refs: HashMap<String, Ref>,
    sites: Vec<Place>,
}

/// A patch while its spec lines are read.
struct Draft {
    line: usize,
    name: String,
    dll: Dll,
    tier: Option<Tier>,
    about: Option<String>,
    sites: Vec<Place>,
    displace: Option<u32>,
    body: Option<(usize, String)>,
    data: Option<u32>,
    refs: HashMap<String, Ref>,
}

impl Draft {
    /// The patch without its body and fixups (at its first site), and where they come from.
    fn finish(self, key: &str) -> Result<(Patch, Source), String> {
        let Draft { line, name, dll, tier, about, sites, displace, body, data, refs } = self;
        let missing = |what: &str| format!("line {line}: patch {name} has no `{what}`");
        let about = about.ok_or_else(|| missing("about"))?;
        let tier = tier.ok_or_else(|| missing("tier"))?;
        let site = sites.first().cloned().ok_or_else(|| missing("site"))?;
        let displace = displace.ok_or_else(|| missing("displace"))?;
        let (body_line, symbol) = body.ok_or_else(|| missing("body"))?;
        let patch = Patch {
            key: key.to_string(),
            tier,
            name,
            about,
            dll,
            site,
            displace,
            body: Vec::new(),
            entry: 0,
            fixups: Vec::new(),
            data: data.unwrap_or(0),
            consts: Vec::new(),
            const_fixups: Vec::new(),
        };
        Ok((patch, Source { line: body_line, symbol, refs, sites }))
    }
}

fn parse_spec(spec: &str, key: &str) -> Result<Vec<(Patch, Source)>, String> {
    let lines: Vec<(usize, &str)> = spec
        .lines()
        .enumerate()
        .map(|(i, text)| (i + 1, text.trim()))
        .filter(|(_, text)| !text.is_empty() && !text.starts_with('#'))
        .collect();
    let mut drafts = Vec::new();
    for (line, text) in lines {
        directive(&mut drafts, line, text).map_err(|e| format!("line {line}: {e}"))?;
    }
    drafts.into_iter().map(|d| d.finish(key)).collect()
}

fn directive(drafts: &mut Vec<Draft>, line: usize, text: &str) -> Result<(), String> {
    let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let args: Vec<&str> = rest.split_whitespace().collect();
    if word == "patch" {
        let [name, file] = args[..] else { bail!("expected `patch <name> <dll>`") };
        let Some(dll) = Dll::ALL.into_iter().find(|dll| dll.file() == file) else {
            bail!("unknown DLL {file} (byondcore.dll or byondwin.dll)");
        };
        if let Some(other) = drafts.iter().find(|d| d.name == name) {
            bail!("patch {name} is already on line {}", other.line);
        }
        drafts.push(Draft {
            line,
            name: name.to_string(),
            dll,
            tier: None,
            about: None,
            sites: Vec::new(),
            displace: None,
            body: None,
            data: None,
            refs: HashMap::new(),
        });
        return Ok(());
    }
    let Some(draft) = drafts.last_mut() else { bail!("`{word}` before any `patch`") };
    match word {
        "about" if rest.is_empty() => bail!("expected `about <text>`"),
        "about" => set(&mut draft.about, rest.to_string(), word),
        "tier" => {
            let tier = Tier::parse(rest).ok_or("expected `tier safe|extended|breaking`")?;
            set(&mut draft.tier, tier, word)
        }
        "site" => {
            draft.sites.push(place(&args)?);
            Ok(())
        }
        "displace" => {
            let [n] = args[..] else { bail!("expected `displace <n>`") };
            set(&mut draft.displace, number(n)?, word)
        }
        "body" => {
            let [symbol] = args[..] else { bail!("expected `body <symbol>`") };
            set(&mut draft.body, (line, symbol.to_string()), word)
        }
        "data" => {
            let [n] = args[..] else { bail!("expected `data <bytes>`") };
            set(&mut draft.data, number(n)?, word)
        }
        "ref" => {
            let [symbol, kind, how @ ..] = &args[..] else { bail!("expected `ref <symbol> code|global|import ...`") };
            if builtin(symbol).is_some() {
                bail!("{symbol} is built in");
            }
            let target = match *kind {
                "code" => Ref::Code(place(how)?),
                "global" => Ref::Global(place(how)?),
                "import" => {
                    let [dll, name] = how else { bail!("expected `ref <symbol> import <dll> <name>`") };
                    Ref::Import { dll: dll.to_string(), name: name.to_string() }
                }
                _ => bail!("unknown ref kind {kind} (code, global, or import)"),
            };
            if draft.refs.insert(symbol.to_string(), target).is_some() {
                bail!("{symbol} already has a `ref` in patch {}", draft.name);
            }
            Ok(())
        }
        _ => bail!("unknown directive `{word}`"),
    }
}

fn set<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    if slot.is_some() {
        bail!("a second `{what}` in one patch");
    }
    *slot = Some(value);
    Ok(())
}

/// `export <name>`, `pattern <offset> <IDA bytes...>`, or `fingerprint <length> <hash> <IDA bytes...>`.
fn place(args: &[&str]) -> Result<Place, String> {
    match args {
        ["export", name] => Ok(Place::Export(name.to_string())),
        ["pattern", at, bytes @ ..] if !bytes.is_empty() => Ok(Place::Pattern(pattern(bytes)?, offset(at)?)),
        ["fingerprint", len, hash, bytes @ ..] if !bytes.is_empty() => {
            let hash = hash.strip_prefix("0x").and_then(|h| u64::from_str_radix(h, 16).ok());
            let hash = hash.ok_or("a fingerprint's hash is `0x` and up to 16 hex digits")?;
            Ok(Place::Fingerprint { prefix: pattern(bytes)?, len: number(len)?, hash })
        }
        _ => bail!(
            "expected `export <name>`, `pattern <offset> <IDA bytes...>`, or `fingerprint <length> <hash> <IDA bytes...>`"
        ),
    }
}

/// IDA bytes as upper-case hex pairs and `??` wildcards, one space apart.
fn pattern(bytes: &[&str]) -> Result<String, String> {
    let bytes = bytes
        .iter()
        .map(|&b| match b {
            "?" | "??" => Ok("??".to_string()),
            _ if b.len() == 2 && b.bytes().all(|c| c.is_ascii_hexdigit()) => Ok(b.to_ascii_uppercase()),
            _ => Err(format!("bad pattern byte {b}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(bytes.join(" "))
}

fn number(text: &str) -> Result<u32, String> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| format!("bad number {text}"))
}

fn offset(text: &str) -> Result<i32, String> {
    let (sign, magnitude) = text.strip_prefix('-').map_or((1, text), |m| (-1, m));
    i32::try_from(number(magnitude)?).map(|n| sign * n).map_err(|_| format!("offset {text} is out of range"))
}

fn builtin(name: &str) -> Option<Ref> {
    match name {
        RESUME => Some(Ref::Resume),
        ORIGINAL => Some(Ref::Original(0)),
        DATA => Some(Ref::Data(0)),
        COM => Some(Ref::Com),
        COM_TAIL => Some(Ref::ComTail),
        _ => None,
    }
}

/// A symbol's name without i686 Windows' leading `_`, and without the `@<bytes>` a stdcall name ends in.
fn plain<'data>(symbol: &impl ObjectSymbol<'data>) -> Option<&'data str> {
    let name = symbol.name().ok()?;
    let Some(c) = name.strip_prefix('_') else { return Some(name) };
    Some(match c.rsplit_once('@') {
        Some((n, bytes)) if !bytes.is_empty() && bytes.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => c,
    })
}

/// Where a symbol at `start` in `section` (`len` bytes) ends: at its size, else the next symbol after it, else the
/// section's end.
fn end_of(file: &CoffFile<'_>, section: SectionIndex, start: u64, size: u64, len: usize) -> u64 {
    match size {
        0 => file
            .symbols()
            .filter(|s| s.section_index() == Some(section) && s.kind() != SymbolKind::Section && s.address() > start)
            .map(|s| s.address())
            .min()
            .unwrap_or(len as u64),
        size => start + size,
    }
}

/// Read-only data a body uses, gathered into the patch's constants: where each symbol's bytes went (and how many), and
/// the addresses in them, which may only point at other constants (a panic's location points at its file name).
#[derive(Default)]
struct Consts {
    bytes: Vec<u8>,
    at: HashMap<SymbolIndex, (u32, u32)>,
    fixups: Vec<(u32, u32)>,
}

impl Consts {
    /// `symbol`'s bytes (a whole section, for a section symbol) in the constants, with what its addresses point at.
    fn add<'data>(&mut self, file: &CoffFile<'data>, symbol: &impl ObjectSymbol<'data>) -> Result<(u32, u32), String> {
        if let Some(&at) = self.at.get(&symbol.index()) {
            return Ok(at);
        }
        let index = symbol.section_index().ok_or("it isn't in a section")?;
        let section = file.section_by_index(index).map_err(|e| e.to_string())?;
        if !matches!(section.kind(), SectionKind::ReadOnlyData | SectionKind::ReadOnlyString) {
            bail!("it is defined in the object outside the code, and not in read-only data");
        }
        let bytes = section.data().map_err(|e| e.to_string())?;
        let (start, end) = match symbol.kind() {
            SymbolKind::Section => (0, bytes.len() as u64),
            _ => (symbol.address(), end_of(file, index, symbol.address(), symbol.size(), bytes.len())),
        };
        let data = bytes.get(start as usize..end as usize).ok_or("it lies outside its section")?;
        self.bytes.resize(self.bytes.len().next_multiple_of(4), 0);
        let at = (self.bytes.len() as u32, data.len() as u32);
        self.bytes.extend_from_slice(data);
        self.at.insert(symbol.index(), at);
        for (pos, relocation) in section.relocations().filter(|(pos, _)| (start..end).contains(pos)) {
            let field = (at.0 + (pos - start) as u32) as usize;
            let RelocationTarget::Symbol(target) = relocation.target() else { bail!("an address without a symbol") };
            if relocation.flags() != (RelocationFlags::Coff { typ: pe::IMAGE_REL_I386_DIR32 }) {
                bail!("it holds a relocation that isn't an address");
            }
            let addend = self.bytes.get(field..field + 4).ok_or("an address past its end")?;
            let addend = u32::from_le_bytes(addend.try_into().unwrap());
            self.bytes[field..field + 4].fill(0);
            let target = file.symbol_by_index(target).map_err(|e| e.to_string())?;
            let (to, len) = self.add(file, &target).map_err(|e| format!("an address in it: {e}"))?;
            if addend > len {
                bail!("an address in it is past its target's {len} bytes");
            }
            self.fixups.push((field as u32, to + addend));
        }
        Ok(at)
    }
}

/// Whether a section holds zero-initialised statics: `.bss`, or `.data` whose bytes are all zero (no relocations).
fn zero_filled<'data>(section: &impl ObjectSection<'data>) -> bool {
    match section.kind() {
        SectionKind::UninitializedData => true,
        SectionKind::Data => {
            section.relocations().next().is_none() && section.data().is_ok_and(|d| d.iter().all(|&b| b == 0))
        }
        _ => false,
    }
}

/// Whether `name` (a symbol the object leaves undefined) is one of core's panic paths: in `core::panicking`, or one of
/// core's `*_fail`/`*_failed` helpers (`slice_index_fail`, `unwrap_failed`, ...). They never return.
fn core_panic(name: &str) -> bool {
    name.contains("4core") && (name.contains("9panicking") || name.contains("_fail"))
}

/// A patch's code: the executable sections its entry reaches (relocated fields not yet zeroed), where
/// [`TRAP`] is in it if core's panic paths go there, and where the zero-initialised statics it uses go in its data.
struct Layout {
    code: Vec<u8>,
    /// Each executable section's offset in the code.
    text: HashMap<SectionIndex, u32>,
    trap: Option<u32>,
    /// Each zero-initialised section the code uses, and its offset in the data.
    zero: HashMap<SectionIndex, u32>,
    /// Where the spec's `data` starts, after the statics.
    zero_len: u32,
}

impl Layout {
    fn new(file: &CoffFile<'_>, entry: SectionIndex) -> Result<Layout, String> {
        let trap = file.symbols().find(|s| s.section_index().is_some() && plain(s) == Some(TRAP));
        // The sections the entry's code reaches through its relocations.
        let (mut reached, mut todo) = (HashSet::from([entry]), vec![entry]);
        let (mut used, mut panics) = (BTreeSet::new(), false);
        while let Some(index) = todo.pop() {
            let section = file.section_by_index(index).map_err(|e| e.to_string())?;
            for (_, relocation) in section.relocations() {
                let RelocationTarget::Symbol(target) = relocation.target() else { continue };
                let Ok(target) = file.symbol_by_index(target) else { continue };
                let to = match target.section_index() {
                    Some(to) => to,
                    None if target.name().is_ok_and(core_panic) => {
                        panics = true;
                        let Some(to) = trap.as_ref().and_then(|t| t.section_index()) else { continue };
                        to
                    }
                    None => continue,
                };
                let kind = file.section_by_index(to).map(|s| (s.kind(), zero_filled(&s))).map_err(|e| e.to_string())?;
                match kind {
                    (SectionKind::Text, _) if reached.insert(to) => todo.push(to),
                    (_, true) => {
                        used.insert(to.0);
                    }
                    _ => {}
                }
            }
        }
        let (mut code, mut text) = (Vec::new(), HashMap::new());
        for section in file.sections().filter(|s| reached.contains(&s.index())) {
            let bytes = section.data().map_err(|e| format!("reading {}: {e}", section.name().unwrap_or("code")))?;
            code.resize(code.len().next_multiple_of(section.align().clamp(1, MAX_ALIGN) as usize), 0xCC);
            text.insert(section.index(), code.len() as u32);
            code.extend_from_slice(bytes);
        }
        let trap = match trap {
            Some(t) if panics => Some(text[&t.section_index().unwrap()] + t.address() as u32),
            _ => None,
        };
        let (mut zero, mut len) = (HashMap::new(), 0u64);
        for index in used {
            let section = file.section_by_index(SectionIndex(index)).map_err(|e| e.to_string())?;
            if section.align() > MAX_ALIGN {
                let name = section.name().unwrap_or("data");
                bail!("{name} wants {}-byte alignment, more than {MAX_ALIGN}", section.align());
            }
            len = len.next_multiple_of(section.align().max(1));
            zero.insert(SectionIndex(index), len as u32);
            len += section.size();
        }
        let zero_len = u32::try_from(len.next_multiple_of(MAX_ALIGN)).map_err(|_| "the statics are too big")?;
        Ok(Layout { code, text, trap, zero, zero_len })
    }
}

/// `pos` in `section` by the function it is in (`name+0x..`), else by the section.
fn place_of(file: &CoffFile<'_>, section: SectionIndex, section_name: &str, pos: u64) -> String {
    let function = file
        .symbols()
        .filter(|s| s.section_index() == Some(section) && s.kind() != SymbolKind::Section && s.address() <= pos)
        .max_by_key(|s| s.address());
    match function.as_ref().and_then(|s| Some((plain(s)?, s.address()))) {
        Some((name, start)) => format!("{name}+{:#x}", pos - start),
        None => format!("{section_name}+{pos:#x}"),
    }
}

/// Reads `patch`'s code (fields zeroed), entry, fixups, constants, and data (its statics, then the spec's `data`)
/// from the object: only the functions its body reaches.
fn read_code(file: &CoffFile<'_>, source: &Source, patch: &mut Patch) -> Result<(), String> {
    let name = source.symbol.as_str();
    let mut found = file
        .symbols()
        .filter(|s| s.kind() != SymbolKind::Section && s.section_index().is_some() && plain(s) == Some(name));
    let Some(symbol) = found.next() else { bail!("no symbol {name} is defined in the object") };
    if found.next().is_some() {
        bail!("more than one symbol is named {name}");
    }
    let section =
        symbol.section_index().filter(|&s| file.section_by_index(s).is_ok_and(|s| s.kind() == SectionKind::Text));
    let Some(section) = section else { bail!("{name} isn't a function") };
    let layout = Layout::new(file, section)?;
    let start = layout.text[&section];
    let spec_data = patch.data;
    let mut code = layout.code.clone();
    let (mut fixups, mut consts) = (Vec::new(), Consts::default());
    for section in file.sections() {
        let Some(&base) = layout.text.get(&section.index()) else { continue };
        let section_name = section.name().unwrap_or("code");
        for (pos, relocation) in section.relocations() {
            let place = place_of(file, section.index(), section_name, pos);
            if pos.checked_add(4).is_none_or(|end| end > section.size()) {
                bail!("the relocation at {place} runs past its section");
            }
            let at = base as usize + pos as usize;
            let addend = crate::pe::u32_at(&code, at).ok_or("internal: a relocation outside the code")? as i32;
            code[at..at + 4].fill(0);
            let field = match relocation.flags() {
                RelocationFlags::Coff { typ: pe::IMAGE_REL_I386_DIR32 } => Field::Abs32,
                RelocationFlags::Coff { typ: pe::IMAGE_REL_I386_REL32 } => Field::Rel32,
                flags => bail!(
                    "the relocation at {place} is {flags:?}; only IMAGE_REL_I386_DIR32 and IMAGE_REL_I386_REL32 are \
                     supported"
                ),
            };
            let RelocationTarget::Symbol(index) = relocation.target() else {
                bail!("the relocation at {place} has no symbol");
            };
            let target = file.symbol_by_index(index).map_err(|e| e.to_string())?;
            let target_name = target.name().unwrap_or("(unnamed)");
            let target = resolve(file, &layout, &mut consts, &target, addend, field, source, spec_data)
                .map_err(|e| format!("the relocation at {place} to {target_name}: {e}"))?;
            fixups.push(Fixup { at: at as u32, field, target });
        }
    }
    fixups.sort_by_key(|f| f.at);
    patch.entry = start + symbol.address() as u32;
    patch.data = layout.zero_len + spec_data;
    (patch.body, patch.fixups, patch.consts, patch.const_fixups) = (code, fixups, consts.bytes, consts.fixups);
    Ok(())
}

/// A relocation to read-only data in the object: the data joins the patch's constants.
fn constant<'data>(
    file: &CoffFile<'data>,
    consts: &mut Consts,
    target: &impl ObjectSymbol<'data>,
    addend: i32,
    field: Field,
) -> Result<Ref, String> {
    if field != Field::Abs32 {
        bail!("a branch to data");
    }
    let (at, len) = consts.add(file, target)?;
    match u32::try_from(addend) {
        Ok(offset) if offset < len => Ok(Ref::Const(at + offset)),
        _ => bail!("offset {addend} is outside its {len} bytes"),
    }
}

/// What a relocation in the code points at.
#[allow(clippy::too_many_arguments)]
fn resolve<'data>(
    file: &CoffFile<'data>,
    layout: &Layout,
    consts: &mut Consts,
    target: &impl ObjectSymbol<'data>,
    addend: i32,
    field: Field,
    source: &Source,
    spec_data: u32,
) -> Result<Ref, String> {
    if let Some(index) = target.section_index() {
        let section = file.section_by_index(index).map_err(|e| e.to_string())?;
        let at = target.address().checked_add_signed(addend.into()).filter(|&at| at < section.size());
        if let Some(&base) = layout.text.get(&index) {
            let Some(at) = at else { bail!("it points outside its section") };
            return Ok(Ref::Body(base + at as u32));
        }
        if let Some(&base) = layout.zero.get(&index) {
            if field != Field::Abs32 {
                bail!("a branch to data");
            }
            let Some(at) = at else { bail!("offset {addend} is outside its section") };
            return Ok(Ref::Data(base + at as u32));
        }
        if section.kind() == SectionKind::Data {
            bail!(
                "it is in {} with bytes that aren't zero or with addresses (only zero-initialised statics can be \
                 written)",
                section.name().unwrap_or("data")
            );
        }
        return constant(file, consts, target, addend, field);
    }
    if !target.is_undefined() {
        bail!("it isn't in a section");
    }
    if let Some(trap) = layout.trap.filter(|_| target.name().is_ok_and(core_panic)) {
        return Ok(Ref::Body(trap));
    }
    let name = plain(target).unwrap_or_default();
    let Some(base) = builtin(name).or_else(|| source.refs.get(name).cloned()) else {
        bail!("not a built-in and no `ref` names it");
    };
    // LLVM writes REL32 as `S + A - (P + 4)`, counting from the field's end like x86, so `jmp sym` has addend 0 and
    // `jmp sym+n` addend n, which only `dmf_original` takes (its instructions from offset n).
    if field == Field::Rel32 && addend != 0 && !matches!(base, Ref::Original(_)) {
        bail!("REL32 addend {addend}, not 0");
    }
    let shift = |offset: i32| offset.checked_add(addend).ok_or_else(|| format!("offset {offset} + {addend} overflows"));
    Ok(match base {
        Ref::Data(_) => match u32::try_from(addend) {
            Ok(offset) if offset < spec_data => Ref::Data(layout.zero_len + offset),
            _ => bail!("offset {addend} is outside the patch's {spec_data} bytes of `data`"),
        },
        Ref::Com | Ref::ComTail if field != Field::Rel32 => {
            bail!("the Direct3D helper can only be called or jumped to")
        }
        Ref::Code(Place::Pattern(bytes, offset)) => Ref::Code(Place::Pattern(bytes, shift(offset)?)),
        Ref::Global(Place::Pattern(bytes, offset)) => Ref::Global(Place::Pattern(bytes, shift(offset)?)),
        Ref::Original(_) => match u32::try_from(addend) {
            Ok(offset) => Ref::Original(offset),
            Err(_) => bail!("offset {addend} is before the original instructions"),
        },
        _ if addend != 0 => bail!("addend {addend} on a target that takes none"),
        base => base,
    })
}

/// What every patch compiles with: `seeker/prelude.rs`.
const PRELUDE: &str = include_str!("../prelude.rs");

/// Compiles a patch folder's `patch.rs` (`patch`) as an i686 COFF object in `out_dir`, after [`PRELUDE`] (its
/// `prelude` module) and with dmfast's `version` as `DMFAST_VERSION`; returns the object's path.
pub fn compile(rustc: &str, patch: &Path, out_dir: &Path, version: &str) -> Result<PathBuf, String> {
    let key = patch.parent().and_then(Path::file_name).ok_or("patch.rs isn't in a folder")?.to_string_lossy();
    let write =
        |path: &Path, text: &str| std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()));
    let prelude = out_dir.join("seeker-prelude.rs");
    write(&prelude, PRELUDE)?;
    let root = out_dir.join(format!("seeker-{key}.rs"));
    write(
        &root,
        &format!("#![no_std]\n#[macro_use]\n#[path = {prelude:?}]\nmod prelude;\n#[path = {patch:?}]\nmod patch;\n"),
    )?;
    let obj = out_dir.join(format!("seeker-{key}.o"));
    let status = Command::new(rustc)
        .args(["--edition", "2024", "--crate-type", "lib", "--crate-name", "seeker_patch", "--emit", "obj"])
        .args(["-C", "opt-level=2", "-C", "panic=abort", "-C", "codegen-units=1"])
        // No jump tables: they are indirect branches.
        .args(["-C", "llvm-args=-min-jump-table-entries=1000000", "--target", "i686-pc-windows-msvc"])
        // A panic's location names its file: by the patch's key, not this machine's paths.
        .arg(format!("--remap-path-prefix={}={key}", patch.parent().unwrap().display()))
        .arg(format!("--remap-path-prefix={}=seeker", out_dir.display()))
        .arg("-o")
        .arg(&obj)
        .arg(&root)
        .env("DMFAST_VERSION", version)
        .status()
        .map_err(|e| format!("running {rustc}: {e}"))?;
    if !status.success() {
        bail!("compiling {} failed", patch.display());
    }
    Ok(obj)
}
