//! `minmax`, Windows: Rust hooks on byondcore's cdecl `max(a, b)` and `min(a, b)`.

use super::{NULL, Tags};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, bail, ensure};

/// The entry bytes the hooks replace: push ebp ; mov ebp,esp ; and a 3-byte instruction ([`entry`] checks).
const PROLOGUE: usize = 6;
/// The list forms' calls of `max` and `min`, in that order (the hooks don't depend on which is which): test al,al ;
/// je ; push [ebp-4] ; push [ebp-8] ; push esi ; push edi ; call pair.
const LIST_CALL: &str = "84 C0 74 ?? FF 75 FC FF 75 F8 56 57 E8";
const LIST_CALL_AT: usize = 12;
/// The switch on `a`'s type, near the entry: cmp eax,57h ; ja default ; movzx eax,byte [eax+index] ;
/// jmp [table+eax*4].
const SWITCH: &str = "83 F8 57 0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85";
const SWITCH_SPAN: usize = 0x30;
const INDEX: usize = 12;
const TABLE: usize = 19;
/// `b`'s type: `[ebp+10h]`.
const B_TYPE: u8 = 0x10;

/// `(a type, a value, b type, b value) -> value` in EDX:EAX.
type Pair = unsafe extern "C" fn(u32, u32, u32, u32) -> u64;
type Hook = extern "C" fn(u32, u32, u32, u32) -> u64;

struct Found {
    tags: Tags,
    max: Foreign<Pair>,
    min: Foreign<Pair>,
}

static FOUND: Installed<Found> = Installed::new();

fn pair(original: &Foreign<Pair>, at: u32, av: u32, bt: u32, bv: u32) -> u64 {
    let Tags { number, text } = FOUND.get().tags;
    let value = |t: u32, v: u32| (u64::from(v) << 32) | u64::from(t);
    // SAFETY: `original` is the hooked function's original code, cdecl with these arguments (checked at install);
    // the arguments are DM values as BYOND passed them.
    unsafe {
        let original = original.get();
        match (at as u8, bt as u8) {
            (NULL, b) if b == number => original(number.into(), 0, bt, bv),
            (a, NULL) if a == number => original(at, av, number.into(), 0),
            (NULL, b) if b == text => value(bt, bv),
            (a, NULL) if a == text => value(at, av),
            _ => original(at, av, bt, bv),
        }
    }
}

extern "C" fn max_hook(at: u32, av: u32, bt: u32, bv: u32) -> u64 {
    pair(&FOUND.get().max, at, av, bt, bv)
}

extern "C" fn min_hook(at: u32, av: u32, bt: u32, bv: u32) -> u64 {
    pair(&FOUND.get().min, at, av, bt, bv)
}

/// `max` and `min`, the functions the list forms call.
fn find(core: &Image) -> Result<[usize; 2]> {
    let calls = core.find_all(LIST_CALL)?;
    let &[max, min] = calls.as_slice() else { bail!("pair max and min: expected 2 list calls, found {}", calls.len()) };
    let pair = [core.call_target(max + LIST_CALL_AT)?, core.call_target(min + LIST_CALL_AT)?];
    ensure!(pair[0] != pair[1], "pair max and min: the list calls call one function");
    Ok(pair)
}

/// The register `mov r32,[ebp+disp]` (`8B /r` with a disp8) loads, if `code` starts with one; only `eax` to `ebx`,
/// which have byte forms.
fn ebp_load(code: [u8; 3], disp: u8) -> Option<u8> {
    let [0x8B, m, x] = code else { return None };
    let r = m >> 3 & 7;
    (m & 0xC7 == 0x45 && r < 4 && x == disp).then_some(r)
}

/// `f`'s entry up to its type switch: push ebp ; mov ebp,esp and a 3-byte `sub esp,x` or `mov r,[ebp+x]` (the bytes
/// the hook moves). Returns the register it loads `b`'s type into, if it does (else each case loads it). The first
/// `skip` bytes are read but not relied on (the hook's, for [`value_tags`]).
fn entry(core: &Image, f: usize, switch: usize, skip: usize) -> Result<Option<u8>> {
    let code = core.haystack(f, switch - f)?;
    core.rely(f + skip, switch - f - skip)?;
    ensure!(
        matches!(code, [0x55, 0x8B, 0xEC, 0x83, 0xEC, _, ..] | [0x55, 0x8B, 0xEC, 0x8B, 0x45, _, ..]),
        "entry not as expected"
    );
    Ok(code.windows(3).find_map(|w| ebp_load([w[0], w[1], w[2]], B_TYPE)))
}

/// The tag the code at `at` compares `b`'s type with: `[mov r,[ebp+10h]] ; [test r8,r8 ; je] ; cmp r8,TAG` (`tested`:
/// with the test), `r` from [`entry`] unless the code loads it; `None` if it doesn't.
fn b_compare(core: &Image, mut at: usize, b: Option<u8>, tested: bool) -> Result<Option<u8>> {
    let loaded = ebp_load(core.array(at)?, B_TYPE);
    if loaded.is_some() {
        at += 3;
    }
    let Some(r) = loaded.or(b) else { return Ok(None) };
    if tested {
        // test r8,r8 ; je (rel8 or rel32)
        if core.array::<2>(at)? != [0x84, 0xC0 | r << 3 | r] {
            return Ok(None);
        }
        at += match core.array(at + 2)? {
            [0x74, _] => 4,
            [0x0F, 0x84] => 8,
            _ => return Ok(None),
        };
    }
    // cmp al,imm8 has its own encoding.
    Ok(match core.array(at)? {
        [0x3C, tag, _] if r == 0 => Some(tag),
        [0x80, op, tag] if op == 0xF8 | r => Some(tag),
        _ => None,
    })
}

/// What `f`'s cases (`a`'s type) compare `b`'s type with: case 0 (null) with NUMBER, unless it returns `b` as it is;
/// the tags whose case does `test b ; je ; cmp b,TAG` with its own tag (number and text).
fn cases(core: &Image, f: usize, skip: usize) -> Result<(Option<u8>, Vec<u8>)> {
    let switch = core.find_unique_in(f, SWITCH_SPAN, SWITCH, "type switch")?;
    let b = entry(core, f, switch, skip)?;
    let (index, table) = (core.ptr_at(switch + INDEX)?, core.ptr_at(switch + TABLE)?);
    let case = |tag: u8| -> Result<usize> {
        let i = core.u8_at(index + usize::from(tag))?;
        core.ptr_at(table + 4 * usize::from(i))
    };
    let number = b_compare(core, case(NULL)?, b, false)?;
    // The switch covers tags 0 to 57h.
    let mut own = Vec::new();
    for t in 1..=0x57 {
        if b_compare(core, case(t)?, b, true)? == Some(t) {
            own.push(t);
        }
    }
    Ok((number, own))
}

/// Both functions' tags, which must agree. The number tag may come from one function's null case alone: the hooks
/// never pass null with a number to the original.
fn tags(core: &Image, [max, min]: [usize; 2], skip: usize) -> Result<Tags> {
    let (max_number, own) = cases(core, max, skip).map_err(|e| format!("pair max: {e}"))?;
    let (min_number, other) = cases(core, min, skip).map_err(|e| format!("pair min: {e}"))?;
    ensure!(own == other, "pair max and min disagree on the type tags");
    let number = match (max_number, min_number) {
        (Some(n), None) | (None, Some(n)) => n,
        (Some(n), Some(m)) if n == m => n,
        _ => bail!("pair max and min: null case not as expected"),
    };
    let text = match own[..] {
        [t, n] | [n, t] if n == number => t,
        _ => bail!("pair max and min: number ({number:#x}) and text cases not as expected"),
    };
    Ok(Tags { number, text })
}

pub(crate) fn value_tags(core: &Image) -> Result<Tags> {
    tags(core, find(core)?, PROLOGUE)
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let [max, min] = find(core)?;
    let tags = tags(core, [max, min], 0)?;
    // cdecl with four dword arguments: some call to each is followed by `add esp,10h` (that call is relied on, the
    // others are not).
    for (f, what) in [(max, "pair max"), (min, "pair min")] {
        let popped = |c: usize| core.bytes(c + 5, 3).is_ok_and(|b| b == [0x83, 0xC4, 0x10]);
        let Some(c) = core.probe_calls_to(f)?.into_iter().find(|&c| popped(c)) else {
            bail!("{what}: no caller cleans up 10h bytes (not cdecl with 4 arguments)");
        };
        core.rely(c, 5)?;
    }
    let found = Found {
        tags,
        max: Foreign::at(core.hook(max, PROLOGUE, max_hook as Hook)?),
        min: Foreign::at(core.hook(min, PROLOGUE, min_hook as Hook)?),
    };
    FOUND.commit(cx, found);
    Ok(core.report(&[max, min]))
}
