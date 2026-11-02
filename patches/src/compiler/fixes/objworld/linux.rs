//! `objworld`, Linux: the queue pass (node in eax or edi) and the var-name dispatch of an entry (node in edx), each
//! `cmp dword [node],13h ; <mid> ; je world`.

use super::{EAX, EDI, EDX, Site, parent_offset, redirect};
use crate::base::code::Cond;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail};

/// The queue pass: cmp dword [eax],13h ; mov [ebp-x],eax ; je world ; test esi,esi ; je
const QUEUE_EAX: &str = "83 38 13 89 45 ?? 0F 84 ?? ?? ?? ?? 85 F6 74";
/// The same in other builds: cmp dword [edi],13h ; je world ; test esi,esi ; je
const QUEUE_EDI: &str = "83 3F 13 0F 84 ?? ?? ?? ?? 85 F6 74";
/// The var-name dispatch: cmp dword [edx],13h ; je world ; cmp eax,0BDh ; ja
const DISPATCH: &str = "83 3A 13 0F 84 ?? ?? ?? ?? 3D BD 00 00 00 0F 87";

/// The type path builder: test eax,eax (node) ; je ; push edi ; push esi ; push ebx ; mov ebx,eax ;
/// mov eax,[eax+parent] ; call itself ; mov esi,[ebx]. Matches that call something else are not relied on.
const BUILDER: &str = "85 C0 0F 84 ?? ?? ?? ?? 57 56 53 89 C3 8B 40 ?? E8 ?? ?? ?? ?? 8B 33";
const PARENT: usize = 15;
const RECURSE: usize = 17;

/// `cmp dword [node],13h ; <mid bytes> ; je world` at `at`.
fn je_world(core: &Image, at: usize, node: u8, mid: usize) -> Result<Site> {
    let je = at + 3 + mid;
    Ok(Site { at, node, copy: mid, jcc: Some((Cond::Equal, core.rel32(je + 2)?)), back: je + 6 })
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut queue = core.find_all(QUEUE_EAX)?.into_iter().map(|at| (at, EAX, 3)).collect::<Vec<_>>();
    queue.extend(core.find_all(QUEUE_EDI)?.into_iter().map(|at| (at, EDI, 0)));
    let [(a, node, mid)] = queue[..] else { bail!("world node test: expected 1 match, found {}", queue.len()) };
    let b = core.find_unique(DISPATCH, "world var dispatch")?;
    let parent = parent_offset(core, BUILDER, PARENT, RECURSE)?;
    redirect(core, je_world(core, a, node, mid)?, parent)?;
    redirect(core, je_world(core, b, EDX, 0)?, parent)?;
    Ok(format!("at +{:#x}, +{:#x}", a - core.base, b - core.base))
}
