//! `filter`, Windows (byondcore). A hash table's bucket array pointer is at +14h (replaced on growth, kept by the
//! compile-start clear). Install checks each offset against the instructions using it.

use std::arch::naked_asm;

use super::{MIN_CHILDREN, init, record, should_skip};
use crate::base::code::{Addr, Foreign};
use crate::base::feature::Ctx;
use crate::base::{Result, bail, ensure};

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
    // Child type dispatch: sub eax,1EDh ; je path_a ; sub eax,0Ah ; je path_b ; sub eax,1 ; jne next ; path_b.
    // [ebp+10h] picks: A (0x1ED) only when 0 (`cmp byte [ebp+10h],0 ; jne next`), B otherwise (`... ; je next`).
    let d = core
        .find_bytes_in(back, body, &[0x2D, 0xED, 0x01, 0x00, 0x00, 0x0F, 0x84])?
        .unique("child type dispatch (0x1ED/0x1F7/0x1F8)")?;
    let path_a = core.rel32(d + 7)?;
    let path_b = d + 25;
    core.expect(d + 11, &[0x83, 0xE8, 0x0A, 0x74], "resolution loop does not look as expected")?;
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
    // The filter relies on the loop marking indexed children: `or dword ptr [esi+14h], 8` right after each indexer
    // call (516). Builds that re-index on every lookup (517 alpha) are not eligible.
    let ors = core.find_bytes_in(back, body, &[0x83, 0x4E, 0x14, 0x08])?;
    let [o0, o1] = ors.as_slice()[..] else {
        bail!("expected 2 indexed-flag stores in the loop, found {}", ors.len());
    };
    // Each then probes for (child, name) with name = `mov r32,[ebp+0Ch]`, the name the entry stub uses.
    for o in [o0, o1] {
        let [op, modrm, disp] = core.array(o + 4)?;
        ensure!(op == 0x8B && modrm & 0xC7 == 0x45 && disp == 0x0C, "resolution loop: probe name not [ebp+0Ch]");
    }
    // Each path then loads its table (mov ecx,<table>) and calls the probe with it.
    let table_after = |o: usize| -> Result<(usize, usize)> {
        let load = core.find_bytes_in(o + 4, 12, &[0xB9])?.first_or("table load")?;
        let call = core.find_bytes_in(load + 5, 0x18, &[0xE8])?.first_or("probe call")?;
        Ok((core.ptr_at(load + 1)?, core.call_target(call)?))
    };
    let (oa, ob) = if o1 > path_a && o0 < path_a { (o1, o0) } else { (o0, o1) };
    ensure!(oa >= path_a && ob <= path_a, "could not tell the two declaration paths apart");
    let [(table_a, probe), (table_b, probe_b)] = [table_after(oa)?, table_after(ob)?];
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
    init(cx, false, tables)?;
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
