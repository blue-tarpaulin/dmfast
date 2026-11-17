//! [`Image`]'s readers: bytes as shipped (relied on) or as they are now, and the branches in them.

use super::{Branch, Image};
use crate::code::Cond;
use crate::ledger::rely;
use crate::{Result, bail, ensure};

impl Image {
    /// The dword at `addr` as it is now, with earlier features' hooks, not relied on.
    pub fn live_u32(&self, addr: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(*self.live_bytes(addr, 4)?.first_chunk().ok_or("short read")?))
    }

    /// The pointer at `addr` as it is now (an import slot's value, perhaps another feature's hook), not relied on.
    pub fn live_ptr(&self, addr: usize) -> Result<usize> {
        self.live_u32(addr).map(|v| v as usize)
    }

    /// `addr..addr+len` as it is now, with earlier features' hooks, not relied on: headers, tables, hooks.
    pub fn live_bytes(&self, addr: usize, len: usize) -> Result<&[u8]> {
        self.check(addr, len)?;
        // SAFETY: mapped (`new`); writes wait for the ledger's commit, after install code drops its borrows.
        Ok(unsafe { std::slice::from_raw_parts(addr as *const u8, len) })
    }

    /// The NUL-terminated string at `addr`, without the NUL, as it is now, not relied on.
    pub fn c_str(&self, addr: usize) -> Result<&[u8]> {
        let &(start, len) = self
            .mapped
            .iter()
            .find(|&&(s, n)| (s..s + n).contains(&addr))
            .ok_or_else(|| format!("{addr:#x} is outside the image"))?;
        let rest = self.live_bytes(addr, start + len - addr)?;
        let end = memchr::memchr(0, rest).ok_or_else(|| format!("no string end after {addr:#x}"))?;
        Ok(&rest[..end])
    }

    /// `addr..addr+len` as shipped (data: as it is now), not relied on: something to search through, or to read
    /// without relying on it.
    pub fn haystack(&self, addr: usize, len: usize) -> Result<&[u8]> {
        self.check(addr, len)?;
        match crate::snapshot::at(addr, len) {
            // SAFETY: the copy outlives every image (`crate::driver::run` drops them before releasing it).
            Some(p) => Ok(unsafe { std::slice::from_raw_parts(p, len) }),
            None => self.live_bytes(addr, len),
        }
    }

    /// `addr..addr+len` as shipped (data: as it is now), relied on.
    pub fn bytes(&self, addr: usize, len: usize) -> Result<&[u8]> {
        let b = self.haystack(addr, len)?;
        rely(addr, len);
        Ok(b)
    }

    /// The `N` bytes at `addr` as shipped, relied on.
    pub fn array<const N: usize>(&self, addr: usize) -> Result<[u8; N]> {
        self.bytes(addr, N)?.first_chunk().copied().ok_or_else(|| "short read".into())
    }

    /// The byte at `addr` as shipped, relied on.
    pub fn u8_at(&self, addr: usize) -> Result<u8> {
        self.array(addr).map(|[b]| b)
    }

    /// The dword at `addr` as shipped, relied on.
    pub fn u32_at(&self, addr: usize) -> Result<u32> {
        self.array(addr).map(u32::from_le_bytes)
    }

    /// The absolute address operand at `addr`, as shipped, relied on.
    pub fn ptr_at(&self, addr: usize) -> Result<usize> {
        self.u32_at(addr).map(|v| v as usize)
    }

    /// Target of the rel32 field at `field` (the instruction ends right after it).
    pub fn rel32(&self, field: usize) -> Result<usize> {
        Ok(field.wrapping_add(4).wrapping_add(self.u32_at(field)? as i32 as isize as usize))
    }

    /// `Err(err)` unless `addr` holds `bytes` as shipped (relied on).
    pub fn expect(&self, addr: usize, bytes: &[u8], err: &'static str) -> Result<()> {
        if self.bytes(addr, bytes.len())? != bytes {
            return Err(err.into());
        }
        Ok(())
    }

    /// The installing feature relies on `addr..addr+len` (a match a probe found, a range read with
    /// [`Image::haystack`]).
    pub fn rely(&self, addr: usize, len: usize) -> Result<()> {
        self.check(addr, len)?;
        rely(addr, len);
        Ok(())
    }

    /// Target of the rel8 field at `field` (the instruction ends right after it).
    pub fn rel8(&self, field: usize) -> Result<usize> {
        let [b] = self.array(field)?;
        Ok((field + 1).wrapping_add(b as i8 as isize as usize))
    }

    /// Target of the `call rel32` at `at`; `Err` if `at` isn't one (`E8`).
    pub fn call_target(&self, at: usize) -> Result<usize> {
        ensure!(self.u8_at(at)? == 0xE8, "{at:#x}: not a call");
        self.rel32(at + 1)
    }

    /// The jump at `at`: `jmp` (`EB`, `E9`) or `j<cc>` (`7x`, `0F 8x`); `Err` if `at` isn't one.
    pub fn branch_at(&self, at: usize) -> Result<Branch> {
        let op = self.u8_at(at)?;
        let (cond, target, next) = match op {
            0xEB => (None, self.rel8(at + 1)?, at + 2),
            0xE9 => (None, self.rel32(at + 1)?, at + 5),
            0x70..=0x7F => (Some(Cond::from_cc(op)), self.rel8(at + 1)?, at + 2),
            0x0F => {
                let op2 = self.u8_at(at + 1)?;
                ensure!(op2 & 0xF0 == 0x80, "{at:#x}: not a jump");
                (Some(Cond::from_cc(op2)), self.rel32(at + 2)?, at + 6)
            }
            _ => bail!("{at:#x}: not a jump"),
        };
        Ok(Branch { target, next, cond })
    }

    /// The `j<cond>` at `at` ([`Image::branch_at`]); `Err` if it's another instruction or condition.
    pub fn jcc_at(&self, at: usize, cond: Cond) -> Result<Branch> {
        let b = self.branch_at(at)?;
        ensure!(b.cond == Some(cond), "{at:#x}: not a {cond:?} jump");
        Ok(b)
    }

    /// `Ok` if the IDA pattern matches at `addr` (relied on), else `Err("{what}: not as expected")`. For exact bytes
    /// with your own message (a string, a prologue), [`Image::expect`].
    pub fn require(&self, addr: usize, ida: &str, what: &str) -> Result<()> {
        ensure!(self.is_at(addr, ida)?, "{what}: not as expected");
        Ok(())
    }

    /// The function the `call` at `at` calls, which must match `ida` ([`Image::call_target`], [`Image::require`]).
    pub fn follow_call(&self, at: usize, ida: &str, what: &str) -> Result<usize> {
        let f = self.call_target(at)?;
        self.require(f, ida, what)?;
        Ok(f)
    }
}
