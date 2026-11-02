//! [`Stub`]: x86 code built for executable memory, and [`trampoline`].

use super::CodeAddr;
use crate::Result;
#[cfg(target_os = "linux")]
use crate::linux::mem as sys;
#[cfg(windows)]
use crate::windows::mem as sys;

/// A condition for [`Stub::jcc`]: the `cc` of `jcc rel32` (`0F 8cc`), named for what it tests after a `cmp a,b`.
#[derive(Clone, Copy, Debug)]
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
/// A short jump inside the stub is written as bytes, its displacement counted by hand ([`Stub::len`] helps).
#[derive(Default)]
pub struct Stub {
    code: Vec<u8>,
    /// Each branch's rel32 field (its offset in `code`) and absolute target.
    branches: Vec<(usize, usize)>,
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

    /// `call target`. An `extern "C"` target takes its arguments on the stack (pushed right to left, popped by the
    /// stub after it), returns in `eax` (`edx:eax`, or `st0` for a float), and clobbers `eax`, `ecx`, `edx`, and the
    /// flags: save what the patched code still needs. On Linux, `esp` must be 16-byte aligned at the `call`.
    pub fn call(&mut self, target: impl CodeAddr) -> &mut Stub {
        self.branch(&[0xE8], target)
    }

    /// `jmp target`.
    pub fn jmp(&mut self, target: impl CodeAddr) -> &mut Stub {
        self.branch(&[0xE9], target)
    }

    /// `j<cond> target` (the rel32 form).
    pub fn jcc(&mut self, cond: Cond, target: impl CodeAddr) -> &mut Stub {
        self.branch(&[0x0F, 0x80 | cond as u8], target)
    }

    fn branch(&mut self, op: &[u8], target: impl CodeAddr) -> &mut Stub {
        self.code.extend_from_slice(op);
        self.branches.push((self.code.len(), target.code_addr()));
        self.code.extend_from_slice(&[0; 4]);
        self
    }

    /// Its length so far: the offset of the next instruction.
    pub fn len(&self) -> usize {
        self.code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// Copy it into executable memory, its branches resolved: its address. Freed again if the installing feature
    /// (or its part) doesn't install.
    pub fn exec(&self) -> Result<usize> {
        let len = self.code.len();
        let p = sys::alloc_exec(len)?;
        let mut code = self.code.clone();
        for &(field, target) in &self.branches {
            code[field..field + 4].copy_from_slice(&(target.wrapping_sub(p + field + 4) as u32).to_le_bytes());
        }
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
