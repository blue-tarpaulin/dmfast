//! [`Image`]'s writers: code and data writes the ledger applies if the installing feature succeeds, jumps, calls,
//! stubs, and hooks.

use super::index::{ends, strictly_between as between};
use super::{Image, within};
use crate::code::{CodeAddr, Stub, branch, trampoline};
use crate::ledger;
use crate::{Result, bail, ensure};

impl Image {
    /// Fails if something else may branch into one of `ranges` (`(at, len)`), code about to be rewritten in place: a
    /// direct branch (`E8`, `E9`, or `0F 8x` rel32) from elsewhere in the code, a relocation inside a range, or a
    /// relocated word (a jump table entry, a function pointer) pointing inside one. A branch or pointer to a range's
    /// start is fine, and so is a branch from inside a range, which is rewritten too. Short branches (`EB`, `7x`) and
    /// computed jumps aren't seen: check the code around a range when they could reach it.
    pub fn untouched(&self, ranges: &[(usize, usize)]) -> Result<()> {
        // Overlaps `lo..hi`; `inside(to, to)`: strictly inside a range.
        let inside = |lo: usize, hi: usize| ranges.iter().any(|&(at, len)| hi > at && lo < at + len);
        let pointer = |r: usize, to: usize| -> Result<()> {
            if inside(to, to) {
                bail!("relocated pointer at {r:#x} into rewritten code at {to:#x}");
            }
            Ok(())
        };
        match self.reloc_index()? {
            Some(relocs) => {
                for &(at, len) in ranges {
                    let first = relocs.sites.partition_point(|&r| r + 4 <= at);
                    if let Some(&r) = relocs.sites.get(first).filter(|&&r| r < at + len) {
                        bail!("relocation at {r:#x} in rewritten code");
                    }
                    if let Some(&e) = between(&relocs.code, at, at + len).first() {
                        let (to, r) = ends(e);
                        pointer(r, to)?;
                    }
                }
                // Data as it is now: a feature's applied write may have changed a word.
                let data = self.hays(Some(false))?;
                for &r in &relocs.data {
                    let to = match data.iter().find(|&&(s, b)| s <= r && r + 4 <= s + b.len()) {
                        Some(&(s, b)) => super::le32(b, r - s).ok_or("short read")? as usize,
                        None => self.word(r)?,
                    };
                    pointer(r, to)?;
                }
            }
            None => {
                for r in self.reloc_sites() {
                    if inside(r, r + 4) {
                        bail!("relocation at {r:#x} in rewritten code");
                    }
                    pointer(r, self.word(r)?)?;
                }
            }
        }
        if let Some(b) = self.branches()? {
            for &(at, len) in ranges {
                if let Some((to, from)) =
                    between(&b.all, at, at + len).iter().map(|&e| ends(e)).find(|&(_, from)| !inside(from, from + 1))
                {
                    bail!("branch at {from:#x} into rewritten code at {to:#x}");
                }
            }
            return Ok(());
        }
        let code = self.hays(Some(true))?;
        for (from, to, _) in crate::scan::branches(&code) {
            if inside(to, to) && !inside(from, from + 1) {
                bail!("branch at {from:#x} into rewritten code at {to:#x}");
            }
        }
        Ok(())
    }

    /// Overwrite code bytes (in the executable parts) when the installing feature succeeds.
    pub fn write_code(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(true, addr, data, false)
    }

    /// [`Image::write_code`] of `jmp target` at `site`, NOP-padded to `len` (>= 5). When `len` covers more than the
    /// one instruction it replaces, check [`Image::untouched`] first.
    pub fn jump(&self, site: usize, len: usize, target: impl CodeAddr) -> Result<()> {
        if len < 5 {
            bail!("jump at {site:#x}: needs 5 bytes, got {len}");
        }
        self.write_code(site, &branch(0xE9, site, target, len))
    }

    /// [`Image::write_code`] of `call target` at `site`, NOP-padded to `len` (>= 5), as [`Image::jump`].
    pub fn call(&self, site: usize, len: usize, target: impl CodeAddr) -> Result<()> {
        if len < 5 {
            bail!("call at {site:#x}: needs 5 bytes, got {len}");
        }
        self.write_code(site, &branch(0xE8, site, target, len))
    }

    /// [`Image::write_code`] of the rel32 field at `field` (a `call`, `jmp`, or `jcc` that ends right after it), so
    /// the branch goes to `target` instead: the write counterpart of [`Image::rel32`].
    pub fn retarget(&self, field: usize, target: impl CodeAddr) -> Result<()> {
        self.write_code(field, &(target.code_addr().wrapping_sub(field + 4) as u32).to_le_bytes())
    }

    /// The conditional jump at `at` always jumps: `7x` becomes `EB`, `0F 8x` becomes `90 E9` (the same target). `Err`
    /// if `at` isn't a conditional jump.
    pub fn always_jump(&self, at: usize) -> Result<()> {
        let b = self.branch_at(at)?;
        ensure!(b.cond.is_some(), "{at:#x}: not a conditional jump");
        self.write_code(at, if b.next - at == 2 { &[0xEB] } else { &[0x90, 0xE9] })
    }

    /// The conditional jump at `at` never jumps: it becomes NOPs. `Err` if `at` isn't a conditional jump.
    pub fn never_jump(&self, at: usize) -> Result<()> {
        let b = self.branch_at(at)?;
        ensure!(b.cond.is_some(), "{at:#x}: not a conditional jump");
        self.write_code(at, &vec![0x90; b.next - at])
    }

    /// [`Image::write_code`] of `stub` resolved to run at `site` ([`Stub::at`]), NOP-padded to `len`; `Err` if it's
    /// longer. When it replaces more than one instruction, check [`Image::untouched`] first.
    pub fn write_stub(&self, site: usize, len: usize, stub: &Stub) -> Result<()> {
        let mut code = stub.at(site)?;
        ensure!(code.len() <= len, "stub at {site:#x}: {} bytes, room for {len}", code.len());
        code.resize(len, 0x90);
        self.write_code(site, &code)
    }

    /// Hook the function at `f` at its entry: its first `len` (>= 5) bytes, whole position-independent instructions,
    /// become `jmp hook`, after [`Image::untouched`] checks that nothing else lands inside them. Returns the original
    /// function: a copy of those bytes, then a jump to the rest ([`trampoline`]). The hook is entered with the
    /// caller's registers and stack, so it has the original's signature and convention (`extern "C"`, `"thiscall"`,
    /// or a naked function), and calls the original the same way (`Foreign::at(original)`).
    pub fn hook(&self, f: usize, len: usize, hook: impl CodeAddr) -> Result<usize> {
        if len < 5 {
            bail!("hook at {f:#x}: needs 5 bytes, got {len}");
        }
        self.untouched(&[(f, len)])?;
        let original = trampoline(self.bytes(f, len)?, f + len)?;
        self.jump(f, len, hook)?;
        Ok(original)
    }

    /// [`Image::write_code`] over another feature's hook that the new code calls through (it is in this one's
    /// `after` list).
    pub fn wrap_code(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(true, addr, data, true)
    }

    /// Overwrite data bytes (in the non-executable parts: a pointer table entry, an import slot) when the installing
    /// feature succeeds. The page keeps its permissions.
    pub fn write_data(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(false, addr, data, false)
    }

    /// [`Image::write_data`] over another feature's write, as [`Image::wrap_code`].
    pub fn wrap_data(&self, addr: usize, data: &[u8]) -> Result<()> {
        self.record(false, addr, data, true)
    }

    /// `"at +0x1a2b, +0x3c4d"`: `sites` as offsets from the image's base, for an install's report line.
    pub fn report(&self, sites: &[usize]) -> String {
        let offsets: Vec<String> = sites.iter().map(|&s| format!("+{:#x}", s - self.base)).collect();
        format!("at {}", offsets.join(", "))
    }

    /// A write inside one executable (`exec`) or non-executable part.
    fn record(&self, exec: bool, addr: usize, data: &[u8], wrap: bool) -> Result<()> {
        if !self.in_parts(exec, addr, data.len()) {
            let what = if exec { "code" } else { "data" };
            bail!("write at {addr:#x}+{:#x}: outside the image's {what}", data.len());
        }
        let poke: ledger::Poke = if exec { crate::poke_code } else { crate::poke_data };
        ledger::record(addr, data, poke, wrap)
    }

    /// `addr..addr+len` lies in one executable (`exec`) or non-executable part.
    fn in_parts(&self, exec: bool, addr: usize, len: usize) -> bool {
        within(self.parts.iter().filter(|p| p.2 == exec).map(|&(a, n, _)| (a, n)), addr, len)
    }
}
