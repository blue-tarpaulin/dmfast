//! `minmax`, Windows: Rust hooks on byondcore's cdecl `max(a, b)` and `min(a, b)`.

use super::{NULL, Tags};
use crate::base::code::Foreign;
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::sync::Installed;
use crate::base::{Result, bail, ensure};

/// The entry bytes the hooks replace: push ebp ; mov ebp,esp ; sub esp,x.
const PROLOGUE: usize = 6;
/// `max` and `min`: push ebp ; mov ebp,esp ; sub esp,x ; push ebx ; mov ebx,[ebp+8] ; movzx eax,bl ; ... ;
/// cmp eax,57h (the switch on `a`'s type follows).
const MAX: &str = "55 8B EC 83 EC 20 53 8B 5D 08 0F B6 C3 8B 5D 10 56 8B 75 0C 57 83 F8 57";
const MIN: &str = "55 8B EC 83 EC 18 53 8B 5D 08 0F B6 C3 8B 5D 10 56 57 8B 7D 0C 83 F8 57";

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

/// `max` and `min`, matched from `skip` bytes in on (`skip` whole bytes of the patterns, 3 characters each).
fn find(core: &Image, skip: usize) -> Result<(usize, usize)> {
    let max = core.find_unique(&MAX[3 * skip..], "pair max")? - skip;
    let min = core.find_unique(&MIN[3 * skip..], "pair min")? - skip;
    Ok((max, min))
}

/// The tags `f` switches on (`b`'s type in `bl`), through `ja default ; movzx eax,byte [eax+index] ;
/// jmp [table+eax*4]`. Case 0 (null) does `mov eax,[ebp+10h] ; cmp al,NUMBER`; the number case and exactly one
/// other, text, do `test bl,bl ; je ; cmp bl,TAG`.
fn tags_of(core: &Image, f: usize) -> Result<Tags> {
    core.require(f + 24, "0F 87 ?? ?? ?? ?? 0F B6 80 ?? ?? ?? ?? FF 24 85", "type switch")?;
    let (index, table) = (core.ptr_at(f + 33)?, core.ptr_at(f + 40)?);
    let case = |tag: u8| -> Result<usize> {
        let entry = core.u8_at(index + usize::from(tag))?;
        core.ptr_at(table + 4 * usize::from(entry))
    };
    let null = case(NULL)?;
    core.require(null, "8B 45 10 3C", "null case")?;
    let number = core.u8_at(null + 4)?;
    let compares_own = |tag: u8| -> Result<bool> {
        let c = case(tag)?;
        let je = match core.bytes(c, 3)? {
            [0x84, 0xDB, 0x74] => 2,
            [0x84, 0xDB, 0x0F] if core.u8_at(c + 3)? == 0x84 => 6,
            _ => return Ok(false),
        };
        Ok(core.bytes(c + 2 + je, 3)? == [0x80, 0xFB, tag])
    };
    ensure!(compares_own(number)?, "number case ({number:#x}) not as expected");
    // The switch covers tags 0 to 57h (the pattern's `cmp eax,57h`).
    let mut text = Vec::new();
    for t in 1..=0x57 {
        if t != number && compares_own(t)? {
            text.push(t);
        }
    }
    let [text] = text[..] else { bail!("text case: expected 1, found {}", text.len()) };
    Ok(Tags { number, text })
}

/// Both functions' tags, which must agree.
fn tags(core: &Image, max: usize, min: usize) -> Result<Tags> {
    let tags = tags_of(core, max).map_err(|e| format!("pair max: {e}"))?;
    let other = tags_of(core, min).map_err(|e| format!("pair min: {e}"))?;
    ensure!(other == tags, "pair max and min disagree on the type tags");
    Ok(tags)
}

pub(crate) fn value_tags(core: &Image) -> Result<Tags> {
    let (max, min) = find(core, PROLOGUE)?;
    tags(core, max, min)
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (max, min) = find(core, 0)?;
    let tags = tags(core, max, min)?;
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
