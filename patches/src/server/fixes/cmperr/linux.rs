//! `cmperr`, Linux: each handler has its own error block, so each `jne skip` goes to its own stub.
//!
//! A handler tests the right operand's type (`cmp al,2Ah ; je load ; mov esi,[state] ; test al,al ; jne skip`). At
//! `load` the left operand is fetched, its type tested, and the handler's error block follows.

use super::HANDLERS;
use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// `cmp al,2Ah ; je load ; mov esi,[state] ; test al,al ; jne skip`. Other code with this head is a candidate, not
/// relied on.
const HEAD: &str = "3C 2A 74 ?? 8B 35 ?? ?? ?? ?? 84 C0 0F 85 ?? ?? ?? ??";
/// The `jne skip`'s rel32.
const SKIP: usize = 14;
/// `load`: the fetch of the left operand (`mov esi,[state] ; movzx eax,[esi+42h] ; mov edx,[esi+3Ch] ;
/// lea ebx,[edx+eax*8-8]`, [`FETCH_LEN`] bytes, absolute addresses only), then `movzx eax,[ebx] ; test al,al ; je ;
/// cmp al,2Ah ; je`.
const LOAD: &str =
    "8B 35 ?? ?? ?? ?? 0F B7 46 42 8B 56 3C 8D 5C C2 F8 0F B6 03 84 C0 0F 84 ?? ?? ?? ?? 3C 2A 0F 84 ?? ?? ?? ??";
const FETCH_LEN: usize = 17;
/// The error block, at [`ERROR_AT`] from `load`.
const ERROR: &str = "83 EC 08 FF B5 ?? ?? ?? ?? FF B5 ?? ?? ?? ?? 68 ?? ?? ?? ?? FF 73 04 FF 33 68 ?? ?? ?? ?? E8";
const ERROR_AT: usize = 0x24;
/// One stub: the fetch, then `jmp error`.
const STUB: usize = FETCH_LEN + 5;

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // (head, load)
    let mut sites = Vec::new();
    for h in core.probe_all(HEAD)? {
        let load = h + 4 + usize::from(core.array::<1>(h + 3)?[0]);
        if load > h + 0x40 || !core.is_at(load, LOAD)? || !core.is_at(load + ERROR_AT, ERROR)? {
            continue;
        }
        core.rely(h, HEAD.split_whitespace().count())?;
        sites.push((h, load));
    }
    if sites.len() < HANDLERS {
        bail!("comparison handlers: expected at least {HANDLERS}, found {}", sites.len());
    }
    let fetches = sites.iter().map(|&(_, load)| core.bytes(load, FETCH_LEN)).collect::<Result<Vec<_>>>()?;
    let stubs = exec_code(STUB * sites.len(), |s| {
        let mut v = Vec::new();
        for (&(_, load), fetch) in sites.iter().zip(&fetches) {
            v.extend(*fetch);
            v.extend(branch(0xE9, s + v.len(), load + ERROR_AT, 5)); // jmp error
        }
        v
    })?;
    for (i, &(h, _)) in sites.iter().enumerate() {
        core.write_code(h + SKIP, &((stubs + STUB * i).wrapping_sub(h + SKIP + 4) as u32).to_le_bytes())?;
    }
    Ok(format!("{} handlers", sites.len()))
}
