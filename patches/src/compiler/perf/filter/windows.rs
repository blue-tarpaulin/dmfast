//! `filter`, Windows (byondcore). A hash table's bucket array pointer is at +14h (replaced on growth, kept by the
//! compile-start clear). Install checks each offset against the instructions using it.

use std::arch::naked_asm;

use super::{Decls, MIN_CHILDREN, init, record, should_skip};
use crate::base::code::{Addr, Foreign};
use crate::base::feature::Ctx;
use crate::base::{Result, bail, ensure};
use crate::compiler::names::Names;

/// Locals of the resolution function (ebp-relative): child count, loop start index.
const LOCAL_COUNT: i8 = -0x1C;
const LOCAL_INDEX: i8 = -0x18;
/// Bucket array pointer offset in a table.
pub(super) const BUCKETS: usize = 0x14;
/// Loop entry: mov ecx,[ebp+18h] | mov ebx,esi ; mov [ecx],al ; test esi,esi ; je fwd
const ENTRY: &str = "8B 4D 18 8B DE 88 01 85 F6 0F 84 ?? ?? ?? ??";
/// An indexer's insert call: mov eax,[ebp+8] ; mov ecx,<table> ; ... ; call insert. Other tables' are candidates,
/// not relied on.
const INSERT: &str = "8B 45 08 B9 ?? ?? ?? ?? 89 45 F8 8B 07 89 45 FC 8D 45 0C 50 8D 45 F8 50 E8 ?? ?? ?? ??";
const INSERT_CALL: usize = 24;
/// A loop path indexing an unmarked child: test byte [esi+14h],8 ; jne probe ; push esi ; push esi ; call indexer ;
/// add esp,8. Then (516) it marks the child: or dword [esi+14h],8.
const INDEX: &str = "F6 46 14 08 75 ?? 56 56 E8 ?? ?? ?? ?? 83 C4 08";
const INDEX_SKIP: usize = 5;
const INDEX_CALL: usize = 8;
const MARK: &str = "83 4E 14 08";
/// An indexer that marks the node it walks: push ebp ; mov ebp,esp ; sub esp,8 ; push ebx ; push edi ;
/// mov edi,[ebp+0Ch] (the node). Its walk of the node's children indexes each, then marks the node: mov eax,[edi+1Ch] ;
/// push [eax+esi*4] ; push [ebp+8] ; call itself ; inc esi ; add esp,8 ; cmp esi,ebx ; jb ; or dword [edi+14h],8
const MARKING: &str = "55 8B EC 83 EC 08 53 57 8B 7D 0C";
const MARKING_WALK: &str = "8B 47 1C FF 34 B0 FF 75 08 E8 ?? ?? ?? ?? 46 83 C4 08 3B F3 72 ?? 83 4F 14 08";
const MARKING_WALK_CALL: usize = 9;

/// BYOND's insert (thiscall).
static INSERT_FN: Addr = Addr::new();
/// The backward child loop, after the entry's `je fwd`.
static BACK_LOOP: Addr = Addr::new();
/// The forward child loop.
static FWD: Addr = Addr::new();
/// Past both loops.
static AFTER: Addr = Addr::new();
/// BYOND's probe: thiscall (table, &{child, name}, default) -> value or default.
static PROBE: Foreign<unsafe extern "thiscall" fn(usize, *const [u32; 2], u32) -> u32> = Foreign::new();

/// Whether table `table` holds key (child, name): BYOND's own probe.
///
/// # Safety
/// `table` is one of the two live tables.
pub(super) unsafe fn has_key(table: usize, child: u32, name: u32) -> bool {
    // SAFETY: PROBE is BYOND's probe, found at install; `table` is live (caller).
    unsafe { PROBE.get()(table, &[child, name], 0) != 0 }
}

/// Replaces `call insert` at an indexer (thiscall, ecx = table, [esp+4] = &{child, name}): records, then inserts.
macro_rules! insert_stub {
    ($name:ident, $t:literal) => {
        /// # Safety
        /// Only called from an indexer's insert call site.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "pushad",
                "mov eax, dword ptr [esp + 36]",
                "push dword ptr [eax + 4]",
                "push dword ptr [eax]",
                "push {t}",
                "call {record}",
                "add esp, 12",
                "popad",
                "jmp dword ptr [{insert}]",
                t = const $t,
                record = sym record,
                insert = sym INSERT_FN,
            )
        }
    };
}
insert_stub!(insert_stub_a, 0);
insert_stub!(insert_stub_b, 1);

/// Replaces `mov ebx,esi ; mov [ecx],al ; test esi,esi ; je fwd` at the loop entry. edi = scope, [ebp+0Ch] = name,
/// [ebp+10h] = selector, [ebp-1Ch] = child count. The skip path sets up what the code after both loops expects:
/// edi = [ebp+8], ebx = 0, esi = start index = max(esi, count).
///
/// # Safety
/// Only jumped to from the loop entry.
#[unsafe(naked)]
unsafe extern "C" fn entry_stub() {
    naked_asm!(
        "mov ebx, esi",
        "mov byte ptr [ecx], al",
        // Scopes below MIN_CHILDREN are never filtered: skip the call entirely.
        "cmp dword ptr [ebp - 0x1C], {min}",
        "jb 8f",
        "pushad",
        "push dword ptr [ebp - 0x1C]",
        "push dword ptr [ebp + 0x10]",
        "push dword ptr [ebp + 0x0C]",
        "push edi",
        "call {skip}",
        "add esp, 16",
        "test eax, eax",
        "popad",
        "jne 4f",
        "8:",
        "test esi, esi",
        "je 3f",
        "jmp dword ptr [{back}]",
        "3:",
        "jmp dword ptr [{fwd}]",
        "4:",
        "mov edi, dword ptr [ebp + 8]",
        "xor ebx, ebx",
        "mov eax, dword ptr [ebp - 0x1C]",
        "cmp esi, eax",
        "jge 5f",
        "mov esi, eax",
        "mov dword ptr [ebp - 0x18], eax",
        "5:",
        "jmp dword ptr [{after}]",
        skip = sym should_skip,
        back = sym BACK_LOOP,
        fwd = sym FWD,
        after = sym AFTER,
        min = const MIN_CHILDREN,
    )
}

/// Locate everything first; patch only if all checks pass.
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let base = core.base;
    let site = core.find_unique(ENTRY, "resolution loop entry")? + 3;
    let back = site + 12;
    let fwd = core.rel32(site + 8)?;
    // fwd: mov esi,[ebp-18h] precedes ; cmp esi,[ebp-1Ch] ; jge after. Before that, the backward loop ends
    // `mov edi,[ebp+8] ; test ebx,ebx ; jne loop`: the state the skip path recreates.
    let err = "unexpected forward-loop header";
    core.expect(fwd - 3, &[0x8B, 0x75, LOCAL_INDEX as u8], err)?;
    core.expect(fwd, &[0x3B, 0x75, LOCAL_COUNT as u8, 0x0F, 0x8D], err)?;
    core.expect(fwd - 14, &[0x8B, 0x7D, 0x08, 0x85, 0xDB], err)?;
    let after = core.rel32(fwd + 5)?;
    let body = fwd - back;
    // Child type dispatch: sub eax,<var> ; je path_a ; sub eax,<verb - var> ; je path_b ; sub eax,1 ; jne next ;
    // path_b. [ebp+10h] picks: A (`var`) only when 0 (`cmp byte [ebp+10h],0 ; jne next`), B otherwise (`... ; je next`).
    let d = core.find_in(back, body, "2D ?? ?? 00 00 0F 84")?.unique("child type dispatch")?;
    let path_a = core.rel32(d + 7)?;
    let path_b = d + 25;
    core.require(d + 11, "83 E8 ?? 74", "child type dispatch")?;
    let var = core.u32_at(d + 1)?;
    let verb = var + u32::from(core.u8_at(d + 13)?);
    let names = Names::new(core)?;
    ensure!(
        [var, verb, verb + 1] == [names.id("var")?, names.id("verb")?, names.id("proc")?],
        "child type dispatch: not var, verb, and proc"
    );
    ensure!(
        core.rel8(d + 15)? == path_b
            && core.bytes(d + 16, 5)? == [0x83, 0xE8, 0x01, 0x0F, 0x85]
            && core.bytes(path_a, 5)? == [0x80, 0x7D, 0x10, 0x00, 0x75]
            && core.bytes(path_b, 6)? == [0x80, 0x7D, 0x10, 0x00, 0x0F, 0x84]
            && core.bytes(back, 3)? == [0x0F, 0x1F, 0x00]
            // children array: mov eax,[edi+1Ch] (edi = scope)
            && core.bytes(back + 3, 3)? == [0x8B, 0x47, 0x1C]
            // parent link: mov ecx,[ecx+18h] ; test ecx,ecx ; jne (the walk up the parent chain)
            && core.find_bytes_in(back, body, &[0x8B, 0x49, 0x18, 0x85, 0xC9, 0x75])?.len() == 1,
        "resolution loop does not look as expected"
    );
    // The filter relies on a child marked indexed having all its keys inserted. Each path indexes an unmarked child
    // ([`INDEX`]), then marks it, or its indexer marks the node it walked once all its keys are in.
    let calls = core.find_in(back, body, INDEX)?;
    let [c0, c1] = calls.as_slice()[..] else {
        bail!("expected 2 indexer calls in the loop, found {}", calls.len());
    };
    let probe_at = |c: usize| -> Result<usize> {
        let next = c + INDEX.split_whitespace().count();
        let at = if core.is_at(next, MARK)? {
            next + MARK.split_whitespace().count()
        } else {
            let f = core.follow_call(c + INDEX_CALL, MARKING, "indexer")?;
            let w = core.find_unique_in(f, 0x100, MARKING_WALK, "indexer: marking walk")?;
            ensure!(core.rel32(w + MARKING_WALK_CALL + 1)? == f, "indexer: the walk is not its own");
            next
        };
        ensure!(core.rel8(c + INDEX_SKIP)? == at, "resolution loop: an indexed child does not skip to its probe");
        // The probe for (child, name), name = `mov r32,[ebp+0Ch]`: the name the entry stub uses.
        let [op, modrm, disp] = core.array(at)?;
        ensure!(op == 0x8B && modrm & 0xC7 == 0x45 && disp == 0x0C, "resolution loop: probe name not [ebp+0Ch]");
        Ok(at)
    };
    let (p0, p1) = (probe_at(c0)?, probe_at(c1)?);
    // Each path then loads its table (mov ecx,<table>) and calls the probe with it.
    let table_after = |p: usize| -> Result<(usize, usize)> {
        let load = core.find_bytes_in(p, 12, &[0xB9])?.first_or("table load")?;
        let call = core.find_bytes_in(load + 5, 0x18, &[0xE8])?.first_or("probe call")?;
        Ok((core.ptr_at(load + 1)?, core.call_target(call)?))
    };
    let (pa, pb) = if p1 > path_a && p0 < path_a { (p1, p0) } else { (p0, p1) };
    ensure!(pa >= path_a && pb <= path_a, "could not tell the two declaration paths apart");
    let [(table_a, probe), (table_b, probe_b)] = [table_after(pa)?, table_after(pb)?];
    let tables = [table_a, table_b];
    // The probe (ecx = table): mov esi,ecx ; ... ; mov eax,[esi+14h] ; shr edx,cl ; mov eax,[eax+edx*4]
    ensure!(
        probe == probe_b
            && !core.find_bytes_in(probe, 0x40, &[0x8B, 0xF1])?.is_empty()
            && !core.find_bytes_in(probe, 0x40, &[0x8B, 0x46, 0x14, 0xD3, 0xEA, 0x8B, 0x04, 0x90])?.is_empty(),
        "probe does not load the bucket array where expected"
    );
    ensure!(table_a != table_b, "both declaration paths use the same table");

    let ins = core.probe_all(INSERT)?;
    let site_for = |t: usize| -> Result<usize> {
        let mut v = Vec::new();
        for &m in &ins {
            if core.ptr_at(m + 4)? == t {
                v.push(m);
            }
        }
        let [m] = v[..] else {
            bail!("table +{:#x}: expected 1 insert site, found {}", t - base, v.len());
        };
        core.rely(m, INSERT.split_whitespace().count())?;
        Ok(m + INSERT_CALL)
    };
    let (call_a, call_b) = (site_for(table_a)?, site_for(table_b)?);
    let insert = core.call_target(call_a)?;
    ensure!(core.call_target(call_b)? == insert, "indexers call different insert functions");

    INSERT_FN.set(insert);
    BACK_LOOP.set(back);
    FWD.set(fwd);
    AFTER.set(after);
    PROBE.set(probe);
    init(cx, false, tables, Decls { a: var, b: [verb, verb + 1] })?;
    core.call(call_a, 5, insert_stub_a as unsafe extern "C" fn())?;
    core.call(call_b, 5, insert_stub_b as unsafe extern "C" fn())?;
    core.jump(site, 12, entry_stub as unsafe extern "C" fn())?;
    Ok(format!(
        "{} (tables +{:#x}/+{:#x}, insert +{:#x})",
        core.report(&[site]),
        table_a - base,
        table_b - base,
        insert - base
    ))
}
