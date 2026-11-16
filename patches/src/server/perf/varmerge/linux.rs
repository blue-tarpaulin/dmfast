//! `varmerge`, Linux: libbyond inlines the merge into its per-type load function; both insert loops get the append
//! check before their search.

use std::arch::naked_asm;

use crate::base::code::{Addr, Cond};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The parent's loop: per var (index [esp+24h], from the list at [esp+2Ch]): name to [esp+28h] ; if the list so far
/// (edi, length in dwords at [esp+20h]) is empty, append ; else search it by the name's order key (table at
/// [esp+18h]: key at [esp+1Ch]) ; append (jbe), replace an equal key, or move the rest up and insert.
const PARENT_LOOP: &str = "0F B7 54 24 24 66 83 7C 24 20 00 89 FB BD 04 00 00 00 8D 04 95 00 00 00 00 89 54 24 3C 89 44 24 \
     30 8B 44 24 2C 8B 40 08 8B 04 90 89 44 24 28 0F 84 ?? ?? ?? ?? 89 C2 8B 44 24 18 31 ED 89 7C 24 \
     14 89 6C 24 10 31 F6 8B 04 90 0F B7 54 24 20 89 44 24 1C EB 17 8D 76 00 0F B7 74 24 08 83 C6 02 \
     0F B7 C6 89 44 24 10 66 39 D6 73 3E 0F B7 C2 03 44 24 10 8B 7C 24 14 D1 F8 89 C1 25 FE FF 00 00 \
     8D 1C 87 8B 7C 24 18 83 E1 FE 8B 2B 66 89 4C 24 08 8B 4C 24 1C 3B 0C AF 77 BE 0F 83 ?? ?? ?? ?? \
     0F B7 54 24 08 66 39 D6 72 C2 8B 7C 24 14 8B 44 24 10 66 89 74 24 08 8D 1C 87 0F B7 54 24 20 8D \
     2C 85 04 00 00 00 66 3B 54 24 08 0F 86 ?? ?? ?? ?? 8B 03 8B 74 24 18 8B 4C 24 1C 3B 0C 86 0F 85 \
     ?? ?? ?? ?? 8B 44 24 28 8B 74 24 30 66 83 44 24 24 02 89 03 8B 44 24 2C 8B 40 08 8B 44 30 04 89 \
     04 2F 0F B7 44 24 24";
/// The type's own loop: the same, from the array at [esp+38h]'s data.
const OWN_LOOP: &str = "8B 44 24 38 66 83 7C 24 20 00 89 FB BD 04 00 00 00 8B 50 08 0F B7 44 24 24 8D 34 85 00 00 00 00 \
     89 44 24 3C 8B 04 82 89 54 24 2C 89 74 24 30 89 44 24 28 0F 84 ?? ?? ?? ?? 89 C2 8B 44 24 18 31 \
     ED 89 7C 24 14 89 6C 24 10 31 F6 8B 04 90 0F B7 54 24 20 89 44 24 1C EB 1B 8D B4 26 00 00 00 00 \
     0F B7 74 24 08 83 C6 02 0F B7 C6 89 44 24 10 66 39 D6 73 3E 0F B7 C2 03 44 24 10 8B 7C 24 14 D1 \
     F8 89 C1 25 FE FF 00 00 8D 1C 87 8B 7C 24 18 83 E1 FE 8B 2B 66 89 4C 24 08 8B 4C 24 1C 3B 0C AF \
     77 BE 0F 83 ?? ?? ?? ?? 0F B7 54 24 08 66 39 D6 72 C2 8B 7C 24 14 8B 44 24 10 66 89 74 24 08 8D \
     1C 87 0F B7 54 24 20 8D 2C 85 04 00 00 00 66 3B 54 24 08 0F 86 ?? ?? ?? ?? 8B 03 8B 74 24 18 8B \
     4C 24 1C 3B 0C 86 0F 85 ?? ?? ?? ?? 8B 44 24 28 8B 74 24 30 66 83 44 24 24 02 89 03 8B 44 24 2C \
     8B 44 30 04 89 04 2F 0F B7 44 24 24";

/// A loop's offsets: the empty-list `je`, the search's start (its first three instructions, mov edx,eax ;
/// mov eax,[esp+18h] ; xor ebp,ebp, are replaced), and the append's store of the var (ebx = its slot, ebp = the value
/// slot's offset from edi).
struct Loop {
    empty_je: usize,
    search: usize,
    append: usize,
}

const PARENT: Loop = Loop { empty_je: 0x2F, search: 0x35, append: 0xE4 };
const OWN: Loop = Loop { empty_je: 0x33, search: 0x39, append: 0xEC };
const SEARCH_LEN: usize = 8;
/// The empty list's path: add word [esp+20h],2 ; jmp append.
const EMPTY_APPEND: &str = "66 83 44 24 20 02 E9";

static PARENT_APPEND: Addr = Addr::new();
static PARENT_SEARCH: Addr = Addr::new();
static OWN_APPEND: Addr = Addr::new();
static OWN_SEARCH: Addr = Addr::new();

/// The stub for one loop: eax = the var's name, edi = the list so far (non-empty), [esp+18h] = the order key table.
macro_rules! append_stub {
    ($name:ident, $append:ident, $search:ident) => {
        /// # Safety
        /// Only jumped to from its loop's search start.
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "movzx ecx, word ptr [esp+0x20]",
                "mov edx, dword ptr [esp+0x18]",
                "mov ebx, dword ptr [edi+ecx*4-8]",
                "mov ebx, dword ptr [edx+ebx*4]",
                "cmp dword ptr [edx+eax*4], ebx",
                "jbe 2f",
                // Above the last key: append, as the search's end would (count += 2, slots past the last pair).
                "lea ebx, [edi+ecx*4]",
                "lea ebp, [ecx*4+4]",
                "add word ptr [esp+0x20], 2",
                "jmp dword ptr [{append}]",
                "2:",
                "mov edx, eax",
                "mov eax, dword ptr [esp+0x18]",
                "xor ebp, ebp",
                "jmp dword ptr [{search}]",
                append = sym $append,
                search = sym $search,
            )
        }
    };
}

append_stub!(parent_stub, PARENT_APPEND, PARENT_SEARCH);
append_stub!(own_stub, OWN_APPEND, OWN_SEARCH);

/// The loop at `at`'s search start and append, checked against its empty-list path.
fn check(core: &Image, at: usize, l: &Loop) -> Result<(usize, usize)> {
    let empty = core.jcc_at(at + l.empty_je, Cond::Equal)?.target;
    core.require(empty, EMPTY_APPEND, "var list merge: empty list append")?;
    ensure!(core.branch_at(empty + 6)?.target == at + l.append, "var list merge: empty list append target");
    Ok((at + l.search, at + l.append))
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let parent = core.find_unique(PARENT_LOOP, "var list merge: parent's loop")?;
    let own = core.find_unique(OWN_LOOP, "var list merge: own loop")?;
    let (parent_search, parent_append) = check(core, parent, &PARENT)?;
    let (own_search, own_append) = check(core, own, &OWN)?;
    PARENT_APPEND.set(parent_append);
    PARENT_SEARCH.set(parent_search + SEARCH_LEN);
    OWN_APPEND.set(own_append);
    OWN_SEARCH.set(own_search + SEARCH_LEN);
    core.jump(parent_search, SEARCH_LEN, parent_stub as *const ())?;
    core.jump(own_search, SEARCH_LEN, own_stub as *const ())?;
    Ok(core.report(&[parent, own]))
}
