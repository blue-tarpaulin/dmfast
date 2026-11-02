//! `dmicmp`, Windows: byondcore's icon state name compare (cdecl) is hooked at entry.

use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::verify;
use crate::base::code::{Foreign, hx};
use crate::base::driver;
use crate::base::feature::Ctx;
use crate::base::sync::Installed;
use crate::base::windows::module_range;
use crate::base::windows::ucrt::ucrt_export;
use crate::base::{Result, bail};

type Compare = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type Char = unsafe extern "C" fn(i32) -> i32;

/// What install found.
pub(super) struct Found {
    original: Foreign<Compare>,
    toupper: Foreign<Char>,
    isalnum: Foreign<Char>,
    /// ucrtbase's `__pctype_func`: the thread's ctype table.
    pctype: Foreign<unsafe extern "C" fn() -> usize>,
    /// ucrtbase's image, where the C locale's tables are.
    ucrt: Range<usize>,
}

static FOUND: Installed<Found> = Installed::new();

/// The answers of `toupper` and `isalnum` for every byte.
struct Tables {
    /// The ctype table they were built under.
    key: usize,
    /// `toupper(b as i8)`'s low byte.
    up: [u8; 256],
    /// `isalnum(up[b]) != 0`.
    alnum: [bool; 256],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

static FAST: AtomicU64 = AtomicU64::new(0);
static FALLBACK: AtomicU64 = AtomicU64::new(0);

/// The compare: push ebp ; mov ebp,esp ; push ebx ; push esi ; mov esi,[ebp+8] ; push edi ; mov edi,[ebp+0Ch] ;
/// cmp esi,edi ; je equal ; test esi,esi ; je less ; test edi,edi ; je greater ;
/// a: movsx eax,byte [esi] ; push eax ; call [toupper] ; mov bh,al ; lea esi,[esi+1] ; add esp,4 ; test bh,bh ;
/// je b ; movzx ecx,bh ; push ecx ; call [isalnum] ; add esp,4 ; test eax,eax ; je a ;
/// b: (the same for edi into bl) ; test bh,bh ; je a_ended ; test bl,bl ; je greater ; cmp bh,bl ; je a ; jl less ;
/// greater: return 1 ; a_ended: test bl,bl ; jne less ; equal: return 0 ; less: return -1
const COMPARE: &str = "55 8B EC 53 56 8B 75 08 57 8B 7D 0C 3B F7 0F 84 7D 00 00 00 85 F6 0F 84 7C 00 00 00 85 FF 74 63 \
                       0F BE 06 50 FF 15 {tu} 8A F8 8D 76 01 83 C4 04 84 FF 74 1A 0F B6 CF 51 FF 15 {ia} 83 C4 04 85 C0 \
                       74 D9 66 0F 1F 84 00 00 00 00 00 0F BE 07 50 FF 15 {tu} 8A D8 83 C4 04 47 84 DB 74 11 0F B6 C3 50 \
                       FF 15 {ia} 83 C4 04 85 C0 74 DB 84 FF 74 14 84 DB 74 06 3A FB 74 9F 7C 15 5F 5E B8 01 00 00 00 5B \
                       5D C3 84 DB 75 07 5F 5E 33 C0 5B 5D C3 5F 5E 83 C8 FF 5B 5D C3";
/// The hooked bytes: push ebp ; mov ebp,esp ; push ebx ; push esi.
const ENTRY: usize = 5;

/// BYOND's compare.
///
/// # Safety
/// `a` and `b` are NUL-terminated strings.
pub(super) unsafe fn original(f: &Found, a: *const u8, b: *const u8) -> i32 {
    // SAFETY: NUL-terminated (caller).
    unsafe { f.original.get()(a, b) }
}

/// The tables, if the thread's locale is the (static) C locale they were built under.
fn tables(f: &Found) -> Option<&'static Tables> {
    // SAFETY: ucrtbase's `__pctype_func`.
    let key = unsafe { f.pctype.get()() };
    if !f.ucrt.contains(&key) {
        return None;
    }
    let t = TABLES.get_or_init(|| {
        // SAFETY: ucrtbase's `toupper` and `isalnum`, on char-range values as BYOND calls them.
        let up: [u8; 256] = std::array::from_fn(|b| unsafe { f.toupper.get()(i32::from(b as u8 as i8)) } as u8);
        // SAFETY: as above.
        let alnum = std::array::from_fn(|b| unsafe { f.isalnum.get()(i32::from(up[b])) } != 0);
        Tables { key, up, alnum }
    });
    (t.key == key).then_some(t)
}

impl Tables {
    /// BYOND's compare, from the tables.
    ///
    /// # Safety
    /// `a` and `b` are NUL-terminated strings.
    unsafe fn compare(&self, mut a: *const u8, mut b: *const u8) -> i32 {
        // The next character kept: uppercased, 0 at the end of the string.
        let next = |p: &mut *const u8| loop {
            // SAFETY: within the string (caller): reading stops at its NUL.
            let byte = usize::from(unsafe { **p });
            // SAFETY: as above.
            *p = unsafe { p.add(1) };
            let c = self.up[byte];
            if c == 0 || self.alnum[byte] {
                break c;
            }
        };
        loop {
            let (ca, cb) = (next(&mut a), next(&mut b));
            if ca == 0 {
                return -i32::from(cb != 0);
            }
            if cb == 0 {
                return 1;
            }
            if ca != cb {
                return (ca as i8).cmp(&(cb as i8)) as i32;
            }
        }
    }
}

/// # Safety
/// `a` and `b` are null or NUL-terminated strings (BYOND passes them).
unsafe extern "C" fn compare(a: *const u8, b: *const u8) -> i32 {
    if a == b {
        return 0;
    }
    if a.is_null() {
        return -1;
    }
    if b.is_null() {
        return 1;
    }
    let f = FOUND.get();
    let Some(t) = tables(f) else {
        FALLBACK.fetch_add(1, Relaxed);
        // SAFETY: NUL-terminated (caller).
        return unsafe { original(f, a, b) };
    };
    FAST.fetch_add(1, Relaxed);
    // SAFETY: NUL-terminated (caller).
    unsafe { verify::check(f, a, b, t.compare(a, b)) }
}

pub(super) fn print_stats() {
    if FOUND.is_set() {
        eprintln!(
            "dmfast: icon state name compare fast={} fallback={} verify={} mismatches={}",
            FAST.load(Relaxed),
            FALLBACK.load(Relaxed),
            driver::verify(),
            verify::MISMATCHES.load(Relaxed)
        );
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (tu, ia) = (core.iat_slot("toupper")?, core.iat_slot("isalnum")?);
    let pattern = COMPARE.replace("{tu}", &hx(tu)).replace("{ia}", &hx(ia));
    let f = core.find_unique(&pattern, "icon state name compare")?;
    let Some((start, end)) = module_range("ucrtbase.dll") else { bail!("ucrtbase.dll not loaded") };
    let ucrt = start..end;
    let (toupper, isalnum) = (core.ptr_at(tu)?, core.ptr_at(ia)?);
    if !ucrt.contains(&toupper) || !ucrt.contains(&isalnum) {
        bail!("toupper/isalnum are not ucrtbase's");
    }
    let pctype = ucrt_export("__pctype_func")?;
    let found = Found {
        original: Foreign::at(core.hook(f, ENTRY, compare as Compare)?),
        toupper: Foreign::at(toupper),
        isalnum: Foreign::at(isalnum),
        pctype: Foreign::at(pctype),
        ucrt,
    };
    FOUND.commit(cx, found);
    Ok(format!("at +{:#x}", f - core.base))
}
