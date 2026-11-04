//! `filter`, Linux (libbyond). A hash table: bucket array pointer, shift byte at buckets-8; a bucket holds a count at
//! -4 and (child, name, value) triples; hash ((child ror 13) ^ name) * 9E3779B9h >> shift. Install checks all of it
//! against the instructions using it.

use std::arch::naked_asm;

use super::{Decls, MIN_CHILDREN, init, record, should_skip};
use crate::base::code::{Addr, hx, live_u32, read};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// The table A probe from its entry to the key compare: indexed test (flags +11h, bit 8), hash, count at -4,
/// 12-byte triples compared as [+4] = name (ebx), [+0] = child (esi). [`has_key`] repeats this.
const PROBE_A: &str = "56 89 C6 53 89 D3 83 EC 04 F6 40 11 08 74 ?? 89 F0 0F B6 0D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? \
                       C1 C8 0D 31 D8 69 C0 B9 79 37 9E D3 E8 8B 04 82 85 C0 74 ?? 8B 48 FC 8D 51 FF 85 C9 75 ?? \
                       EB ?? 66 90 83 C0 0C 83 EA 01 72 ?? 3B 58 04 75 F3 3B 30 75 EF";
/// The table B probe: child in ebx (ror 13 kept at [esp+4]), name in esi, triples [+4] = name, [+0] = child.
const PROBE_B: &str = "55 57 56 89 D6 53 89 C3 89 DF C1 CF 0D 83 EC 2C 0F B6 40 11 89 7C 24 04 A8 08 74 ?? \
                       0F B6 0D ?? ?? ?? ?? 8B 44 24 04 8B 15 ?? ?? ?? ?? 31 F0 69 C0 B9 79 37 9E D3 E8 8B 04 82 \
                       85 C0 0F 84 ?? ?? ?? ?? 8B 48 FC 8D 51 FF 85 C9 75 ?? E9 ?? ?? ?? ?? 8D B4 26 00 00 00 00 \
                       83 C0 0C 83 EA 01 0F 82 ?? ?? ?? ?? 3B 70 04 75 EF 3B 18";
/// Per scope: ... ; mov edx,[ebp+0Ch] ; mov [edx],al ; test edi,edi ; je fwd ; next. 516.1665+: next =
/// cmp byte [ebp-2Ah],0 ; jne; fwd = cmp [ebp-24h],edi ; jle after. Both builds keep scope, name, count, and selector at
/// [ebp-1Ch], [ebp-20h], [ebp-24h], and byte [ebp-29h].
const ENTRY: &str = "0F B6 45 D3 8D 77 FF 83 F0 01 3B 55 E4 0F 95 C2 21 D0 80 7D D5 00 0F 95 C2 21 D0 0F B6 55 D4 \
                     83 F2 01 21 D0 8B 55 0C 88 02 85 FF 0F 84 ?? ?? ?? ?? 80 7D D6 00 0F 85";
/// 516.1660: next = cmp byte [ebp-29h],0 ; je; fwd = mov eax,[ebp-24h] ; test eax,eax ; je after.
const ENTRY_1660: &str = "0F B6 45 D4 8D 77 FF 83 F0 01 3B 55 E4 0F 95 C2 21 D0 80 7D D6 00 0F 95 C2 21 D0 0F B6 55 D5 \
                          83 F2 01 21 D0 8B 55 0C 88 02 85 FF 0F 84 ?? ?? ?? ?? 80 7D D7 00 0F 84";
/// `test edi,edi ; je fwd` in [`ENTRY`].
const SITE: usize = 0x29;
/// Bucket array pointer offset in a table.
pub(super) const BUCKETS: usize = 0;

/// The backward child loop, after the entry's `je fwd`.
static BACK_LOOP: Addr = Addr::new();
/// The forward child loop.
static FWD: Addr = Addr::new();
/// Past both loops.
static AFTER: Addr = Addr::new();
/// The free-list globals the insert sites load into eax, per table.
static FREELIST_A: Addr = Addr::new();
static FREELIST_B: Addr = Addr::new();

/// Whether table `table` holds key (child, name), probing as BYOND does.
///
/// # Safety
/// `table` is one of the two live tables.
pub(super) unsafe fn has_key(table: usize, child: u32, name: u32) -> bool {
    // SAFETY: a live table (caller): its shift byte, buckets, and triples.
    unsafe {
        let buckets = live_u32(table) as usize;
        let shift = u32::from(read::<u8>(table - 8));
        let h = (child.rotate_right(13) ^ name).wrapping_mul(0x9E37_79B9) >> shift;
        let bucket = live_u32(buckets + 4 * h as usize) as usize;
        if bucket == 0 {
            return false;
        }
        (0..live_u32(bucket - 4) as usize)
            .any(|i| live_u32(bucket + 12 * i) == child && live_u32(bucket + 12 * i + 4) == name)
    }
}

/// Replaces `test edi,edi ; je fwd` at the child loop entry: [ebp-1Ch] = scope, [ebp-20h] = name, [ebp-24h] = count,
/// byte [ebp-29h] = selector (0 = table A); only esi and edi are live. The skip path leaves edi = count, as the loops
/// do.
///
/// # Safety
/// Only jumped to from the child loop entry.
#[unsafe(naked)]
unsafe extern "C" fn entry_stub() {
    naked_asm!(
        // Scopes below MIN_CHILDREN are never filtered.
        "cmp dword ptr [ebp - 0x24], {min}",
        "jb 8f",
        "movzx ecx, byte ptr [ebp - 0x29]",
        "mov ebx, esp",
        "and esp, -16",
        "push dword ptr [ebp - 0x24]",
        "push ecx",
        "push dword ptr [ebp - 0x20]",
        "push dword ptr [ebp - 0x1c]",
        "call {skip}",
        "mov esp, ebx",
        "test eax, eax",
        "jne 4f",
        "8:",
        "test edi, edi",
        "je 3f",
        "jmp dword ptr [{back}]",
        "3:",
        "jmp dword ptr [{fwd}]",
        "4:",
        "mov edi, dword ptr [ebp - 0x24]",
        "jmp dword ptr [{after}]",
        skip = sym should_skip,
        back = sym BACK_LOOP,
        fwd = sym FWD,
        after = sym AFTER,
        min = const MIN_CHILDREN,
    )
}

/// Replaces `mov eax,[free-list]` right after an insert stored its triple at `[eax]`: record it, then do that load.
/// Flags are dead here (`test eax,eax` follows).
macro_rules! insert_stub {
    ($name:ident, $t:literal, $freelist:ident) => {
        /// # Safety
        /// Only called from an insert site.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "push ebp",
                "mov ebp, esp",
                "push ecx",
                "push edx",
                "and esp, -16",
                "sub esp, 4",
                "push dword ptr [eax + 4]",
                "push dword ptr [eax]",
                "push {t}",
                "call {record}",
                "lea esp, [ebp - 8]",
                "pop edx",
                "pop ecx",
                "pop ebp",
                "mov eax, dword ptr [{fl}]",
                "mov eax, dword ptr [eax]",
                "ret",
                t = const $t,
                record = sym record,
                fl = sym $freelist,
            )
        }
    };
}
insert_stub!(insert_stub_a, 0, FREELIST_A);
insert_stub!(insert_stub_b, 1, FREELIST_B);

/// Insert sites of the table at `buckets`: `mov [eax+8],reg` (last triple store) then `mov eax,[buckets+4]`,
/// checked against the table's insert counter (`add 1` to [buckets-10h]) so no insert path is missed.
fn insert_sites(core: &Image, buckets: usize, what: &str) -> Result<Vec<usize>> {
    let mut sites = Vec::new();
    for m in core.find_all(&format!("89 ?? 08 A1 {} 85 C0", hx(buckets + 4)))? {
        if core.u8_at(m + 1)? & 0xC7 == 0x40 {
            sites.push(m + 3);
        }
    }
    let counter = hx(buckets - 0x10);
    let increments = core.find_all(&format!("A1 {counter} 83 C0 01 A3 {counter}"))?.len();
    ensure!(
        !sites.is_empty() && sites.len() == increments,
        "{what}: {} insert sites but {increments} insert counter updates",
        sites.len()
    );
    Ok(sites)
}

/// The bucket-array global a probe reads: its first `mov edx,[imm32]`.
fn probe_table(core: &Image, probe: usize, what: &str) -> Result<usize> {
    let load = core.find_bytes_in(probe, 0x60, &[0x8B, 0x15])?.first_or(&format!("{what}: bucket load"))?;
    core.ptr_at(load + 2)
}

/// Locate everything first; patch only if all checks pass.
pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (site, after, b_has_a) = match core.find_unique(ENTRY, "resolution loop entry") {
        Ok(m) => {
            let site = m + SITE;
            let fwd = core.rel32(site + 4)?;
            // fwd: cmp [ebp-24h],edi ; jle after ; cmp byte [ebp-2Ah],0 ; jne ... The backward loop also tests the
            // selector the entry stub reads: cmp byte [ebp-29h],0.
            let err = "unexpected forward-loop header";
            core.expect(fwd, &[0x39, 0x7D, 0xDC, 0x7E], err)?;
            core.expect(fwd + 5, &[0x80, 0x7D, 0xD6, 0x00], err)?;
            core.expect(site + 0x12, &[0x80, 0x7D, 0xD7, 0x00], err)?;
            (site, core.rel8(fwd + 4)?, false)
        }
        Err(e) => {
            let m =
                core.find_unique(ENTRY_1660, "resolution loop entry (516.1660)").map_err(|e2| format!("{e}; {e2}"))?;
            let site = m + SITE;
            let fwd = core.rel32(site + 4)?;
            // fwd: mov eax,[ebp-24h] ; test eax,eax ; je after ; cmp byte [ebp-29h],0
            let err = "unexpected forward-loop header (516.1660)";
            core.expect(fwd, &[0x8B, 0x45, 0xDC, 0x85, 0xC0, 0x0F, 0x84], err)?;
            core.expect(fwd + 11, &[0x80, 0x7D, 0xD7, 0x00], err)?;
            // Here the table-B loops probe `var` children against table B as well.
            (site, core.rel32(fwd + 7)?, true)
        }
    };
    let back = site + 8;
    let fwd = core.rel32(site + 4)?;
    let lo = fwd.min(back);
    // The child loops: table A (`var`, probe A) and table B (`verb` or `proc`, probe B). The match and the probe.
    let probe_after = |ida: &str, what: &str| -> Result<(usize, usize)> {
        let m = core.find_in(lo, after + 0x1000 - lo, ida)?.first_or(what)?;
        Ok((m, core.call_target(m + ida.split_whitespace().count() - 1)?))
    };
    // cmp [eax],1EDh ; jne next ; mov edx,[ebp-20h] ; call probe_a
    let (a, probe_a) = probe_after("81 38 ED 01 00 00 75 ?? 8B 55 E0 E8", "table A probe")?;
    // sub eax,1F7h ; cmp eax,1 ; ja next ; mov edx,[ebp-20h] ; mov eax,ecx ; call probe_b
    let (b, probe_b) = probe_after("2D F7 01 00 00 83 F8 01 77 ?? 8B 55 E0 89 C8 E8", "table B probe")?;
    let verb = core.u32_at(b + 1)?;
    let decls = Decls { a: core.u32_at(a + 2)?, b: [verb, verb + 1] };
    // The flags test, hash, and buckets `should_skip` and `has_key` rely on.
    ensure!(
        core.is_at(probe_a, PROBE_A)? && core.is_at(probe_b, PROBE_B)?,
        "probes do not test the indexed flag or hash as expected"
    );
    let tables = [probe_table(core, probe_a, "table A")?, probe_table(core, probe_b, "table B")?];
    ensure!(tables[0] != tables[1], "both probes use the same table");
    // Each probe hashes with its table's shift byte: movzx ecx,byte [buckets-8].
    for (probe, table) in [(probe_a, tables[0]), (probe_b, tables[1])] {
        let shift = [&[0x0F, 0xB6, 0x0D][..], &((table - 8) as u32).to_le_bytes()].concat();
        ensure!(!core.find_bytes_in(probe, 0x60, &shift)?.is_empty(), "probe does not read its shift where expected");
    }
    // The child loops read the scope's children array (mov eax,[ebp-1Ch] ; mov eax,[eax+18h]), and the code after
    // them walks a child's parent chain (mov eax,[eax+14h] ; sub edx,1F7h).
    let children = core.find_bytes_in(lo, after - lo, &[0x8B, 0x45, 0xE4, 0x8B, 0x40, 0x18])?;
    let parent =
        core.find_bytes_in(lo, after + 0x1000 - lo, &[0x8B, 0x40, 0x14, 0x81, 0xEA, 0xF7, 0x01, 0x00, 0x00])?;
    if children.is_empty() || parent.is_empty() {
        bail!("children array or parent link not where expected");
    }
    let sites = [insert_sites(core, tables[0], "table A")?, insert_sites(core, tables[1], "table B")?];

    BACK_LOOP.set(back);
    FWD.set(fwd);
    AFTER.set(after);
    FREELIST_A.set(tables[0] + 4);
    FREELIST_B.set(tables[1] + 4);
    init(cx, b_has_a, tables, decls)?;
    let stubs: [(unsafe extern "C" fn(), &Vec<usize>); 2] = [(insert_stub_a, &sites[0]), (insert_stub_b, &sites[1])];
    for (stub, sites) in stubs {
        for &s in sites {
            core.call(s, 5, stub)?;
        }
    }
    core.jump(site, 8, entry_stub as unsafe extern "C" fn())?;
    Ok(format!("{} ({}+{} insert sites)", core.report(&[site]), sites[0].len(), sites[1].len()))
}
