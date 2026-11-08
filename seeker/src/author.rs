//! Private patches as their author writes them, turned into [`Patch`]es: an i686 COFF object whose naked functions
//! are the bodies ([`compile`]), and a spec that says where each patch goes and what each outside symbol its body
//! uses is. The spec has one directive per line (blank lines and lines starting with `#` are skipped); a `patch` line
//! starts a patch and the lines after it describe that patch:
//! ```text
//! patch <name> <byondcore.dll|byondwin.dll>
//! tier <safe|extended|breaking>
//! about <text>
//! site export <decorated name>
//! site pattern <offset> <IDA bytes...>
//! displace <n>
//! body <symbol>
//! data <bytes>
//! ref <symbol> code|global export <name>
//! ref <symbol> code|global pattern <offset> <IDA bytes...>
//! ref <symbol> import <dll> <name>
//! ```
//! `about`, `tier`, `site`, `displace`, and `body` are required; `data` defaults to 0; numbers are decimal or `0x`
//! hex. `site` may repeat: the same body at several sites (patches `<name>`, `<name>-2`, ...). `dmf_resume`,
//! `dmf_original`, and `dmf_data` need no `ref`. Names are matched without the `_` i686 Windows puts on C names.
//!
//! A body's relocations become its fixups (`DIR32`: `Abs32`, `REL32`: `Rel32`), the in-place value an addend: to a
//! label in the body, to a `dmf_data` or pattern offset, or to a read-only `static`, whose bytes join the patch's
//! constants.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use object::read::coff::CoffFile;
use object::{
    Architecture, Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionIndex, SectionKind,
    SymbolIndex, SymbolKind, pe,
};

use crate::format::Field;
use crate::patch::{Dll, Fixup, Patch, Place, Ref, Tier};

macro_rules! bail {
    ($($t:tt)*) => {
        return Err(format!($($t)*))
    };
}

const RESUME: &str = "dmf_resume";
const ORIGINAL: &str = "dmf_original";
const DATA: &str = "dmf_data";

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
        read_body(&file, &source, &mut patch)
            .map_err(|e| format!("line {}: patch {}: {e}", source.line, patch.name))?;
        for (i, site) in source.sites.into_iter().enumerate() {
            let name = if i == 0 { patch.name.clone() } else { format!("{}-{}", patch.name, i + 1) };
            patches.push(Patch { name, site, ..patch.clone() });
        }
    }
    Ok(patches)
}

/// Where a patch's body comes from: its `body` line and symbol, and its `ref`s by symbol; and its sites.
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
            fixups: Vec::new(),
            data: data.unwrap_or(0),
            consts: Vec::new(),
        };
        Ok((patch, Source { line: body_line, symbol, refs, sites }))
    }
}

fn parse_spec(spec: &str, key: &str) -> Result<Vec<(Patch, Source)>, String> {
    let mut drafts = Vec::new();
    for (i, text) in spec.lines().enumerate() {
        let text = text.trim();
        if !text.is_empty() && !text.starts_with('#') {
            directive(&mut drafts, i + 1, text).map_err(|e| format!("line {}: {e}", i + 1))?;
        }
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

/// `export <name>` or `pattern <offset> <IDA bytes...>`.
fn place(args: &[&str]) -> Result<Place, String> {
    match args {
        ["export", name] => Ok(Place::Export(name.to_string())),
        ["pattern", at, bytes @ ..] if !bytes.is_empty() => Ok(Place::Pattern(pattern(bytes)?, offset(at)?)),
        _ => bail!("expected `export <name>` or `pattern <offset> <IDA bytes...>`"),
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
        ORIGINAL => Some(Ref::Original),
        DATA => Some(Ref::Data(0)),
        _ => None,
    }
}

/// A symbol's name without i686 Windows' leading `_`.
fn plain<'data>(symbol: &impl ObjectSymbol<'data>) -> Option<&'data str> {
    let name = symbol.name().ok()?;
    Some(name.strip_prefix('_').unwrap_or(name))
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

/// Read-only data a body uses, gathered into the patch's constants: where each symbol's bytes went, and how many.
#[derive(Default)]
struct Consts {
    bytes: Vec<u8>,
    at: HashMap<SymbolIndex, (u32, u32)>,
}

impl Consts {
    /// `symbol`'s bytes (a whole section, for a section symbol) in the constants.
    fn add<'data>(&mut self, file: &CoffFile<'data>, symbol: &impl ObjectSymbol<'data>) -> Result<(u32, u32), String> {
        if let Some(&at) = self.at.get(&symbol.index()) {
            return Ok(at);
        }
        let index = symbol.section_index().ok_or("it isn't in a section")?;
        let section = file.section_by_index(index).map_err(|e| e.to_string())?;
        if !matches!(section.kind(), SectionKind::ReadOnlyData | SectionKind::ReadOnlyString) {
            bail!("it is defined in the object outside the body, and not in read-only data");
        }
        if section.relocations().next().is_some() {
            bail!("its section has relocations (constants can't hold addresses)");
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
        Ok(at)
    }
}

/// Reads `patch`'s body (fields zeroed), fixups, and constants from the object.
fn read_body(file: &CoffFile<'_>, source: &Source, patch: &mut Patch) -> Result<(), String> {
    let data = patch.data;
    let name = source.symbol.as_str();
    let mut found = file.symbols().filter_map(|symbol| {
        let section = symbol.section_index()?;
        (symbol.kind() != SymbolKind::Section && plain(&symbol) == Some(name)).then_some((symbol, section))
    });
    let Some((symbol, section)) = found.next() else { bail!("no symbol {name} is defined in the object") };
    if found.next().is_some() {
        bail!("more than one symbol is named {name}");
    }
    let code_section = file.section_by_index(section).map_err(|e| format!("reading {name}'s section: {e}"))?;
    let bytes = code_section.data().map_err(|e| format!("reading {name}'s section: {e}"))?;
    let start = symbol.address();
    let end = end_of(file, section, start, symbol.size(), bytes.len());
    let Some(mut code) = bytes.get(start as usize..end as usize).map(<[u8]>::to_vec) else {
        bail!("{name} lies outside its section");
    };
    if code.is_empty() {
        bail!("{name} is empty");
    }
    let (mut fixups, mut consts) = (Vec::new(), Consts::default());
    for (pos, relocation) in code_section.relocations().filter(|&(pos, _)| (start..end).contains(&pos)) {
        let at = (pos - start) as usize;
        let Some(addend) = crate::pe::u32_at(&code, at).map(|a| a as i32) else {
            bail!("the relocation at {name}+{at:#x} runs past its end");
        };
        code[at..at + 4].fill(0);
        let field = match relocation.flags() {
            RelocationFlags::Coff { typ: pe::IMAGE_REL_I386_DIR32 } => Field::Abs32,
            RelocationFlags::Coff { typ: pe::IMAGE_REL_I386_REL32 } => Field::Rel32,
            flags => bail!(
                "the relocation at {name}+{at:#x} is {flags:?}; only IMAGE_REL_I386_DIR32 and IMAGE_REL_I386_REL32 \
                 are supported"
            ),
        };
        let RelocationTarget::Symbol(index) = relocation.target() else {
            bail!("the relocation at {name}+{at:#x} has no symbol");
        };
        let target = file.symbol_by_index(index).map_err(|e| e.to_string())?;
        let target_name = target.name().unwrap_or("(unnamed)");
        let elsewhere = !target.is_undefined() && target.section_index().is_some_and(|s| s != section);
        let target = if elsewhere {
            constant(file, &mut consts, &target, addend, field)
        } else {
            resolve(&target, section, start..end, addend, field, source, data)
        }
        .map_err(|e| format!("the relocation at {name}+{at:#x} to {target_name}: {e}"))?;
        fixups.push(Fixup { at: at as u32, field, target });
    }
    fixups.sort_by_key(|f| f.at);
    (patch.body, patch.fixups, patch.consts) = (code, fixups, consts.bytes);
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

/// What a relocation in the body at `body` (offsets in `section`) points at.
fn resolve<'data>(
    target: &impl ObjectSymbol<'data>,
    section: SectionIndex,
    body: std::ops::Range<u64>,
    addend: i32,
    field: Field,
    source: &Source,
    data: u32,
) -> Result<Ref, String> {
    if target.section_index() == Some(section) {
        let at = target.address().checked_add_signed(addend.into());
        return match at.filter(|at| body.contains(at)) {
            Some(at) => Ok(Ref::Body((at - body.start) as u32)),
            None => bail!("it points outside the body"),
        };
    }
    if !target.is_undefined() {
        bail!("it is defined in the object outside the body");
    }
    // LLVM writes REL32 as `S + A - (P + 4)`, counting from the field's end like x86, so its addend is 0.
    if field == Field::Rel32 && addend != 0 {
        bail!("REL32 addend {addend}, not 0");
    }
    let name = plain(target).unwrap_or_default();
    let Some(base) = builtin(name).or_else(|| source.refs.get(name).cloned()) else {
        bail!("not a built-in and no `ref` names it");
    };
    let shift = |offset: i32| offset.checked_add(addend).ok_or_else(|| format!("offset {offset} + {addend} overflows"));
    Ok(match base {
        Ref::Data(_) => match u32::try_from(addend) {
            Ok(offset) if offset < data => Ref::Data(offset),
            _ => bail!("offset {addend} is outside the patch's {data} bytes of data"),
        },
        Ref::Code(Place::Pattern(bytes, offset)) => Ref::Code(Place::Pattern(bytes, shift(offset)?)),
        Ref::Global(Place::Pattern(bytes, offset)) => Ref::Global(Place::Pattern(bytes, shift(offset)?)),
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
        .args(["-C", "opt-level=2", "-C", "panic=abort", "--target", "i686-pc-windows-msvc", "-o"])
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
