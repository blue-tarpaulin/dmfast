//! [`Stub`]: x86 code built for executable memory or for writing in place, and [`trampoline`].

use super::CodeAddr;
#[cfg(target_os = "linux")]
use crate::linux::mem as sys;
#[cfg(windows)]
use crate::windows::mem as sys;
use crate::{Result, bail};

/// A jump condition: the `cc` of `j<cc>` (`7x rel8`, `0F 8x rel32`), named for what it tests after a `cmp a,b`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Cond {
    /// `jo`.
    Overflow = 0,
    /// `jno`.
    NoOverflow,
    /// `jb`, `jc`: unsigned `a < b`.
    Below,
    /// `jae`, `jnc`: unsigned `a >= b`.
    AboveOrEqual,
    /// `je`, `jz` ([`Cond::Zero`]).
    Equal,
    /// `jne`, `jnz` ([`Cond::NotZero`]).
    NotEqual,
    /// `jbe`: unsigned `a <= b`.
    BelowOrEqual,
    /// `ja`: unsigned `a > b`.
    Above,
    /// `js`.
    Sign,
    /// `jns`.
    NoSign,
    /// `jp`.
    Parity,
    /// `jnp`.
    NoParity,
    /// `jl`: signed `a < b`.
    Less,
    /// `jge`: signed `a >= b`.
    GreaterOrEqual,
    /// `jle`: signed `a <= b`.
    LessOrEqual,
    /// `jg`: signed `a > b`.
    Greater,
}

// `Zero` and `NotZero` are named like the variants they alias.
#[allow(non_upper_case_globals)]
impl Cond {
    /// `jz`, after a `test`: [`Cond::Equal`].
    pub const Zero: Cond = Cond::Equal;
    /// `jnz`, after a `test`: [`Cond::NotEqual`].
    pub const NotZero: Cond = Cond::NotEqual;

    /// The condition with code `cc` (its low 4 bits), as in a jcc's opcode.
    pub const fn from_cc(cc: u8) -> Cond {
        use Cond::*;
        [
            Overflow,
            NoOverflow,
            Below,
            AboveOrEqual,
            Equal,
            NotEqual,
            BelowOrEqual,
            Above,
            Sign,
            NoSign,
            Parity,
            NoParity,
            Less,
            GreaterOrEqual,
            LessOrEqual,
            Greater,
        ][(cc & 0xF) as usize]
    }
}

/// An x86 stub being built for executable memory: position-independent bytes, and `call`, `jmp`, and `jcc` to
/// absolute targets, which [`Stub::exec`] resolves once it knows where the stub lives. Write one instruction per call,
/// with its asm as a comment:
///
/// ```ignore
/// let stub = Stub::new()
///     .bytes(&[0x83, 0xF8, 0xFF]) // cmp eax,-1
///     .jcc(Cond::Equal, skip)
///     .bytes(&[0x50]) // push eax
///     .call(hook as unsafe extern "C" fn(u32) -> u32)
///     .bytes(&[0x83, 0xC4, 0x04]) // add esp,4
///     .jmp(back)
///     .exec()?;
/// ```
///
/// Each branch goes to a [`Target`]: an address, or a [`Label`] in the stub (`let done = s.label();`,
/// `.jcc8(Cond::NotEqual, done)`, later `.bind(done)`). An instruction with an embedded dword (an address, an immediate):
/// [`Stub::bytes_u32`].
#[derive(Default)]
pub struct Stub {
    code: Vec<u8>,
    /// Each branch: its displacement field (offset in `code`), width (1 or 4), and destination.
    branches: Vec<(usize, u8, Dest)>,
    /// Each label's offset, once bound.
    labels: Vec<Option<usize>>,
}

/// A place in a [`Stub`] to jump to, from [`Stub::label`]; bound to the next instruction by [`Stub::bind`].
#[derive(Clone, Copy, Debug)]
pub struct Label(usize);

/// Where a [`Stub`] branch goes: a [`Label`] in it, or a code address ([`CodeAddr`]: a `usize`, a fn pointer).
pub trait Target {
    #[doc(hidden)]
    fn dest(self) -> Dest;
}

#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum Dest {
    Abs(usize),
    Label(Label),
}

impl<T: CodeAddr> Target for T {
    fn dest(self) -> Dest {
        Dest::Abs(self.code_addr())
    }
}

impl Target for Label {
    fn dest(self) -> Dest {
        Dest::Label(self)
    }
}

impl Stub {
    pub fn new() -> Stub {
        Stub::default()
    }

    /// Position-independent bytes, as they are.
    pub fn bytes(&mut self, code: &[u8]) -> &mut Stub {
        self.code.extend_from_slice(code);
        self
    }

    /// One instruction with an embedded dword: `prefix`, `v` little-endian, then `suffix` (e.g.
    /// `.bytes_u32(&[0x81, 0x7E, 0x04], cb, &[])` for `cmp dword [esi+4],cb`).
    pub fn bytes_u32(&mut self, prefix: &[u8], v: u32, suffix: &[u8]) -> &mut Stub {
        self.code.extend_from_slice(prefix);
        self.code.extend_from_slice(&v.to_le_bytes());
        self.code.extend_from_slice(suffix);
        self
    }

    /// `call target`. An `extern "C"` target takes its arguments on the stack (pushed right to left, popped by the
    /// stub after it), returns in `eax` (`edx:eax`, or `st0` for a float), and clobbers `eax`, `ecx`, `edx`, and the
    /// flags: save what the patched code still needs. On Linux, `esp` must be 16-byte aligned at the `call`.
    pub fn call(&mut self, target: impl Target) -> &mut Stub {
        self.branch(&[0xE8], 4, target)
    }

    /// `jmp target` (`E9 rel32`).
    pub fn jmp(&mut self, target: impl Target) -> &mut Stub {
        self.branch(&[0xE9], 4, target)
    }

    /// `j<cond> target` (`0F 8x rel32`).
    pub fn jcc(&mut self, cond: Cond, target: impl Target) -> &mut Stub {
        self.branch(&[0x0F, 0x80 | cond as u8], 4, target)
    }

    /// `jmp short target` (`EB rel8`); resolving fails if it's out of reach.
    pub fn jmp8(&mut self, target: impl Target) -> &mut Stub {
        self.branch(&[0xEB], 1, target)
    }

    /// `j<cond> short target` (`7x rel8`); resolving fails if it's out of reach.
    pub fn jcc8(&mut self, cond: Cond, target: impl Target) -> &mut Stub {
        self.branch(&[0x70 | cond as u8], 1, target)
    }

    /// A new label, not bound yet.
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    /// Bind `l` to the next instruction. A label is bound once.
    pub fn bind(&mut self, l: Label) -> &mut Stub {
        assert!(self.labels[l.0].is_none(), "label bound twice");
        self.labels[l.0] = Some(self.code.len());
        self
    }

    fn branch(&mut self, op: &[u8], width: u8, target: impl Target) -> &mut Stub {
        self.code.extend_from_slice(op);
        self.branches.push((self.code.len(), width, target.dest()));
        self.code.extend(std::iter::repeat_n(0, usize::from(width)));
        self
    }

    /// Its length so far: the offset of the next instruction.
    pub fn len(&self) -> usize {
        self.code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// Its bytes for running at `addr`, branches resolved: code to write in place ([`crate::image::Image::write_stub`])
    /// or into executable memory. `Err` for an unbound label or a short jump out of reach.
    pub fn at(&self, addr: usize) -> Result<Vec<u8>> {
        let mut code = self.code.clone();
        for &(field, width, target) in &self.branches {
            let target = match target {
                Dest::Abs(a) => a,
                Dest::Label(l) => addr + self.labels[l.0].ok_or_else(|| format!("stub: label {} not bound", l.0))?,
            };
            let rel = target.wrapping_sub(addr + field + usize::from(width)) as i32;
            if width == 1 {
                let Ok(rel) = i8::try_from(rel) else { bail!("stub: short jump at +{field:#x} out of reach ({rel})") };
                code[field] = rel as u8;
            } else {
                code[field..field + 4].copy_from_slice(&rel.to_le_bytes());
            }
        }
        Ok(code)
    }

    /// Copy it into executable memory, its branches resolved: its address. Freed again if the installing feature
    /// (or its part) doesn't install.
    pub fn exec(&self) -> Result<usize> {
        let len = self.code.len();
        let p = sys::alloc_exec(len)?;
        let code = match self.at(p) {
            Ok(code) => code,
            Err(e) => {
                // SAFETY: `p` is `len` fresh bytes nothing uses yet.
                unsafe { sys::free_exec(p, len) };
                return Err(e);
            }
        };
        // SAFETY: `p` is `len` fresh writable bytes nothing else uses until this returns; freed only on failure.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, len);
            if let Err(e) = sys::seal_exec(p, len) {
                sys::free_exec(p, len);
                return Err(e);
            }
        }
        // SAFETY: runs only if the feature that wanted it doesn't install: nothing runs or points into it.
        crate::ledger::on_abort(Box::new(move || unsafe { sys::free_exec(p, len) }));
        Ok(p)
    }
}

/// Executable copy of position-independent `code` followed by `jmp back`: instructions moved out of the way of an
/// in-place rewrite (check [`crate::image::Image::untouched`] first). For a function's entry use `Image::hook`, which
/// checks it too.
pub fn trampoline(code: &[u8], back: usize) -> Result<usize> {
    Stub::new().bytes(code).jmp(back).exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_absolute_targets_resolve() {
        let mut s = Stub::new();
        let (back, fwd) = (s.label(), s.label());
        s.bind(back)
            .bytes(&[0x90]) // 0: nop
            .jcc8(Cond::NotEqual, fwd) // 1: jne short fwd
            .jmp(back) // 3: jmp back
            .bytes(&[0x90]) // 8: nop
            .bind(fwd)
            .jmp(0x1000usize); // 9: jmp 1000h
        let code = s.at(0x2000).unwrap();
        assert_eq!(code[..9], [0x90, 0x75, 0x06, 0xE9, 0xF8, 0xFF, 0xFF, 0xFF, 0x90]);
        assert_eq!(code[9..], [0xE9, 0xF2, 0xEF, 0xFF, 0xFF]);
    }

    #[test]
    fn short_jump_out_of_reach_fails() {
        let mut s = Stub::new();
        let far = s.label();
        s.jmp8(far).bytes(&[0x90; 128]).bind(far);
        assert!(s.at(0).is_err());
    }

    #[test]
    fn unbound_label_fails() {
        let mut s = Stub::new();
        let l = s.label();
        s.jcc(Cond::Equal, l);
        assert!(s.at(0).is_err());
    }
}
