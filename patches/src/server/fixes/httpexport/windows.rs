//! `httpexport`, Windows: each reference count update jumps to a stub running it with `lock`, then back.

use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;
use crate::base::{Result, ensure};

/// `mov dword [eax+14h],0 ; inc dword [edi+4] ; push eax ; call add_task`: a chunk task is filled in and takes a
/// reference.
const POST: &str = "C7 40 14 00 00 00 00 FF 47 04 50 E8";
/// A task's destructor dropping its reference: `mov ecx,[esi+8] ; mov [esi],task_vtable ;
/// add dword [ecx+4],-1 ; jne past_delete`. The site is the `add`, at +9.
const TASK: &str = "8B 4E 08 C7 06 ?? ?? ?? ?? 83 41 04 FF 75 06 8B 01 6A 01 FF 10 F6 45 08 01 C7 06";
/// The request's owner dropping its reference after its finish handler: `mov ecx,esi ; call [eax+4] ;
/// add dword [esi+4],-1 ; jne past_delete`. The site is the `add`, at +5.
const RELEASE: &str = "8B CE FF 50 04 83 46 04 FF 75 08 8B 06 8B CE 6A 01 FF 10";
const LOCK: u8 = 0xF0;

/// Every match of `ida` plus `offset`; there must be `n`.
fn sites(cx: &Ctx, ida: &str, offset: usize, n: usize, what: &str) -> Result<Vec<usize>> {
    let hits = cx.core().find_all(ida)?;
    ensure!(hits.len() == n, "{what}: expected {n} matches, found {}", hits.len());
    Ok(hits.iter().map(|&at| at + offset).collect())
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    // HttpRequest2's per chunk, and WorldExportHttpRequest's finish handler posting itself from another thread.
    let posts = sites(cx, POST, 0, 2, "HTTP chunk task post")?;
    // The tasks' destructors, then the owner letting go on completion, cancel, and error.
    let mut drops = sites(cx, TASK, 9, 2, "HTTP chunk task destructor")?;
    drops.extend(sites(cx, RELEASE, 5, 3, "HTTP request release")?);
    let mut stubs = Vec::new();
    for at in posts {
        // The `mov`, then the `inc` with `lock`.
        let mut s = Stub::new();
        s.bytes(core.bytes(at, 7)?).bytes(&[LOCK]).bytes(core.bytes(at + 7, 3)?).jmp(at + 10);
        stubs.push((at, 10, s));
    }
    for at in drops {
        // The `add` with `lock`, then its `jne`.
        let past_delete = core.jcc_at(at + 4, Cond::NotEqual)?.target;
        let mut s = Stub::new();
        s.bytes(&[LOCK]).bytes(core.bytes(at, 4)?).jcc(Cond::NotEqual, past_delete).jmp(at + 6);
        stubs.push((at, 6, s));
    }
    core.untouched(&stubs.iter().map(|&(at, len, _)| (at, len)).collect::<Vec<_>>())?;
    for (at, len, stub) in &stubs {
        core.jump(*at, *len, stub.exec()?)?;
    }
    Ok(format!("{} sites", stubs.len()))
}
