//! A DreamSeeker patch as written, before it is placed in a DLL: where it goes, its body's bytes, and what each of
//! the body's address fields points at, by name or pattern (resolved when it is applied).

/// The DLL a patch goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dll {
    Core,
    Win,
}

impl Dll {
    pub const ALL: [Dll; 2] = [Dll::Core, Dll::Win];

    pub fn file(self) -> &'static str {
        match self {
            Dll::Core => "byondcore.dll",
            Dll::Win => "byondwin.dll",
        }
    }
}

/// How safe a patch is to turn on, as for dmfast's bug fixes: a fix selection naming a tier takes every patch at it
/// or a safer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Safe,
    Extended,
    Breaking,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Safe, Tier::Extended, Tier::Breaking];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Safe => "safe",
            Tier::Extended => "extended",
            Tier::Breaking => "breaking",
        }
    }

    pub fn parse(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// A place in the DLL's code or data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// An export, by its (decorated) name.
    Export(String),
    /// The single match of an IDA-style pattern (`??` wildcards) in the DLL's code, plus an offset.
    Pattern(String, i32),
    /// The one function that starts with `prefix` (IDA-style), is `len` bytes long, and whose bytes hash to `hash`
    /// ([`fnv1a`](dmfast_format::fingerprint::fnv1a), skipping relocated addresses and branches out of it): a whole function
    /// checked without keeping its bytes.
    Fingerprint { prefix: String, len: u32, hash: u64 },
}

/// What a body's 4-byte field points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    /// An offset in the code (a function or a label).
    Body(u32),
    /// An offset in the patch's own data.
    Data(u32),
    /// An offset in the patch's read-only constants (an `Abs32` field).
    Const(u32),
    /// The instruction after the site.
    Resume,
    /// The site's original instructions from this offset (an instruction start), then the instruction after the site.
    Original(u32),
    /// A function in the same DLL.
    Code(Place),
    /// BYOND's data in the same DLL (an `Abs32` field).
    Global(Place),
    /// The DLL's existing import slot for `dll!name` (an `Abs32` field, as in `call dword ptr [slot]`).
    Import { dll: String, name: String },
    /// The Direct3D helper's `dmf_com` (a `Rel32` field, `com.rs`).
    Com,
    /// The Direct3D helper's `dmf_com_tail` (a `Rel32` field).
    ComTail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixup {
    /// The field's offset in the body.
    pub at: u32,
    pub field: dmfast_format::manifest::Field,
    pub target: Ref,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    /// Its folder (`patches-private/client/<key>/`), which a fix selection names it by; several patches may share one.
    pub key: String,
    pub tier: Tier,
    pub name: String,
    /// What it fixes, for the audit's report.
    pub about: String,
    pub dll: Dll,
    /// Where it hooks: these `displace` bytes (whole instructions; a relative branch only to the site's end) become a
    /// `jmp` to its gate.
    pub site: Place,
    pub displace: u32,
    /// IDA patterns dreamseeker.exe's code must each have once: the client code the patch is written against, so a
    /// BYOND build that changes it (fixing the bug there) gets no patch.
    pub exe: Vec<String>,
    /// Its code: the functions its entry reaches, in order. Entered at `entry` in DreamSeeker in place of the site's
    /// instructions, with every register and flag as at the site.
    pub body: Vec<u8>,
    pub entry: u32,
    pub fixups: Vec<Fixup>,
    /// Bytes of zero-filled data it owns.
    pub data: u32,
    /// Read-only data it uses (strings, tables), in the DLL's data section.
    pub consts: Vec<u8>,
    /// The addresses in [`consts`](Self::consts): each field's offset there, and the offset there it points at.
    pub const_fixups: Vec<(u32, u32)>,
}

impl Patch {
    /// Whether a fix selection (tiers, keys, `-key` to leave one out; empty: none) turns it on.
    pub fn wanted(&self, entries: &[String]) -> bool {
        let has = |e: &str| entries.iter().any(|x| x == e);
        if entries.iter().any(|e| e.strip_prefix('-') == Some(self.key.as_str())) {
            return false;
        }
        has(&self.key) || Tier::ALL.iter().any(|&t| t >= self.tier && has(t.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection() {
        let p = |key: &str, tier| Patch {
            key: key.into(),
            tier,
            name: key.into(),
            about: String::new(),
            dll: Dll::Core,
            site: Place::Export(String::new()),
            displace: 5,
            exe: Vec::new(),
            body: Vec::new(),
            entry: 0,
            fixups: Vec::new(),
            data: 0,
            consts: Vec::new(),
            const_fixups: Vec::new(),
        };
        let on = |p: &Patch, sel: &str| {
            p.wanted(&sel.split(',').filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>())
        };
        let (safe, ext) = (p("welcome", Tier::Safe), p("rsc", Tier::Extended));
        assert!(!on(&safe, "") && !on(&ext, ""), "nothing is on unless selected");
        assert!(on(&safe, "safe") && !on(&ext, "safe"));
        assert!(on(&safe, "extended") && on(&ext, "extended") && on(&ext, "breaking"));
        assert!(on(&ext, "rsc") && !on(&safe, "rsc"));
        assert!(!on(&safe, "extended,-welcome") && on(&ext, "extended,-welcome"));
    }
}
