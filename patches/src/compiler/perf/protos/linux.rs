//! `protos`, Linux: the lookup-or-insert's one call site (`find(tree, holder, *inserted) -> node`, cdecl) calls
//! [`find`], and the tree's unlink call calls [`unlink`], which also drops the node from the index (the pool frees a
//! block once all its nodes are unlinked).

use std::sync::atomic::Ordering::Relaxed;

use super::{EVICTED, INDEX, lookup, record};
use crate::base::code::{Foreign, hx};
use crate::base::feature::Ctx;
use crate::base::sync::lock;
use crate::base::{Result, bail};

type Find = unsafe extern "C" fn(usize, usize, *mut u8) -> usize;
type Unlink = unsafe extern "C" fn(usize, usize) -> usize;

static ORIGINAL_FIND: Foreign<Find> = Foreign::new();
static ORIGINAL_UNLINK: Foreign<Unlink> = Foreign::new();

/// The call: sub esp,4 ; push esi ; push ebx ; push <tree> ; call find ; add esp,10h ; cmp byte [esp+1Fh],0
const FIND_CALL: &str = "83 EC 04 56 53 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 80 7C 24 1F 00";
const CALL: usize = 10;
/// The lookup itself: push ebp ; push edi ; push esi ; push ebx ; sub esp,1Ch ; mov eax,[esp+30h] (tree) ;
/// mov esi,[eax] (root) ; test esi,esi ; je ... ; mov eax,[esp+34h] (holder) ; mov eax,[eax+1Ch] (key array) ;
/// movzx edi,word [eax+6] (count)
const LOOKUP: &str = "55 57 56 53 83 EC 1C 8B 44 24 30 8B 30 85 F6 0F 84 ?? ?? ?? ?? 8B 44 24 34 8B 40 1C 0F B7 78 06";
/// The unlink call: sub esp,8 ; push eax (node) ; push <tree> ; call unlink ; add esp,10h ; cmp word [eax+18h],0 ;
/// mov ebp,eax
const UNLINK_CALL: &str = "83 EC 08 50 68 {tree} E8 ?? ?? ?? ?? 83 C4 10 66 83 78 18 00 89 C5";
const UNLINK_AT: usize = 9;

/// `node` is being unlinked from its tree.
fn evict(node: usize) {
    let mut index = lock(&INDEX);
    if let Some(slot) = index.keys.remove(&(node as u32)) {
        index.nodes.remove(&slot);
        EVICTED.fetch_add(1, Relaxed);
    }
}

/// # Safety
/// BYOND's call: `tree` and `holder` are live, `inserted` writable.
unsafe extern "C" fn find(tree: usize, holder: usize, inserted: *mut u8) -> usize {
    // SAFETY: the caller's tree, holder, and flag; BYOND's lookup.
    unsafe {
        let node = lookup(tree, holder);
        if node != 0 {
            *inserted = 0;
            return node;
        }
        let node = ORIGINAL_FIND.get()(tree, holder, inserted);
        record(tree, holder, node);
        node
    }
}

/// # Safety
/// BYOND's call: `node` is in `tree`.
unsafe extern "C" fn unlink(tree: usize, node: usize) -> usize {
    evict(node);
    // SAFETY: the caller's tree and node; BYOND's unlink.
    let removed = unsafe { ORIGINAL_UNLINK.get()(tree, node) };
    if removed != node {
        evict(removed);
    }
    removed
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let site = core.find_unique(FIND_CALL, "proto find call")? + CALL;
    let tree = core.u32_at(site - 4)?;
    let f = core.rel32(site + 1)?;
    if core.find_unique(LOOKUP, "proto tree lookup")? != f {
        bail!("proto find call does not target the tree lookup");
    }
    let pattern = UNLINK_CALL.replace("{tree}", &hx(tree as usize));
    let unlink_site = core.find_unique(&pattern, "proto unlink call")? + UNLINK_AT;
    ORIGINAL_FIND.set(f);
    ORIGINAL_UNLINK.set(core.rel32(unlink_site + 1)?);
    core.call(site, 5, find as Find)?;
    core.call(unlink_site, 5, unlink as Unlink)?;
    Ok(format!("at +{:#x}", f - core.base))
}
