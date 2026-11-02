//! `minmax`, Linux: stubs at libbyond's `max(a, b)` and `min(a, b)`, which take the result address in `eax` (also
//! returned), `a`'s type and value in `edx` and `ecx`, and `b`'s at `[esp+4]` and `[esp+8]` (the caller cleans up).

use super::{NULL, Tags};
use crate::base::code::{Cond, Stub};
use crate::base::feature::Ctx;
use crate::base::image::Image;
use crate::base::{Result, bail, ensure};

/// The entry bytes the stubs replace and run: push ebp ; mov ebp,ecx ; push edi ; mov edi,edx.
const PROLOGUE: usize = 6;
/// `max` and `min`: push ebp ; mov ebp,ecx ; push edi ; mov edi,edx ; push esi ; mov esi,eax ; push ebx ;
/// mov ebx,edx ; sub esp,x ; movzx eax,byte [esp+b type] ; mov [esp+x],ecx ; mov edx,[esp+b value] ; mov [esp+x],al ;
/// cmp bl,NUMBER (`a`'s type).
const PATTERN: &str = "55 89 CD 57 89 D7 56 89 C6 53 89 D3 83 EC ?? 0F B6 84 24 ?? 00 00 00 89 4C 24 ?? \
                       8B 94 24 ?? 00 00 00 88 44 24 ?? 80 FB 2A";

/// The tags a match `f` of [`PATTERN`] tests: number is its `cmp bl,2Ah`, then `je number ; jbe below`; `below`
/// does `test bl,bl ; je ; cmp bl,TEXT`. Also checks the stub's assumption that `b` is the first two stack arguments.
fn tags_of(core: &Image, f: usize) -> Result<Tags> {
    let frame = u32::from(core.u8_at(f + 14)?) + 0x10; // sub esp,x and four pushes
    ensure!(
        core.u32_at(f + 19)? == frame + 4 && core.u32_at(f + 30)? == frame + 8,
        "b is not read from the first two stack arguments"
    );
    let number = core.u8_at(f + 40)?;
    core.expect(f + 41, &[0x0F, 0x84], "type tests not as expected")?;
    core.expect(f + 47, &[0x76], "type tests not as expected")?;
    let below = core.rel8(f + 48)?;
    let je = match core.bytes(below, 3)? {
        [0x84, 0xDB, 0x74] => 2,
        [0x84, 0xDB, 0x0F] if core.u8_at(below + 3)? == 0x84 => 6,
        _ => bail!("null test not as expected"),
    };
    core.expect(below + 2 + je, &[0x80, 0xFB], "text test not as expected")?;
    Ok(Tags { number, text: core.u8_at(below + 4 + je)? })
}

/// `max` and `min`, in either order, matched from `skip` bytes in on (`skip` whole bytes of [`PATTERN`], 3
/// characters each).
fn find(core: &Image, skip: usize) -> Result<(usize, usize)> {
    let found = core.find_all(&PATTERN[3 * skip..])?;
    let [first, second] = found.as_slice()[..] else {
        bail!("pair max/min: expected 2 matches, found {}", found.len());
    };
    Ok((first - skip, second - skip))
}

/// Both functions' tags, which must agree.
fn tags(core: &Image, first: usize, second: usize) -> Result<Tags> {
    let tags = tags_of(core, first).map_err(|e| format!("pair max/min at {first:#x}: {e}"))?;
    let other = tags_of(core, second).map_err(|e| format!("pair max/min at {second:#x}: {e}"))?;
    ensure!(other == tags, "pair max and min disagree on the type tags");
    Ok(tags)
}

pub(crate) fn value_tags(core: &Image) -> Result<Tags> {
    let (first, second) = find(core, PROLOGUE)?;
    tags(core, first, second)
}

/// The stub for the function at `f`, whose first [`PROLOGUE`] bytes are `prologue`.
fn stub(f: usize, prologue: &[u8], tags: Tags) -> Result<usize> {
    let mut s = Stub::new();
    let (b_number, a_number, a_other, original) = (s.label(), s.label(), s.label(), s.label());
    s.bytes(&[0x80, 0xFA, NULL]) // cmp dl,0 ; a is null
        .jcc(Cond::NotEqual, a_number) // jne a_number
        .bytes(&[0x80, 0x7C, 0x24, 0x04, tags.number]) // cmp byte [esp+4],NUMBER
        .jcc(Cond::NotEqual, b_number) // jne b_number
        .bytes(&[0xBA, tags.number, 0x00, 0x00, 0x00]) // mov edx,NUMBER ; (null, number): a becomes number 0
        .bytes(&[0x31, 0xC9]) // xor ecx,ecx
        .jmp(original) // jmp original
        .bind(b_number)
        .bytes(&[0x80, 0x7C, 0x24, 0x04, tags.text]) // cmp byte [esp+4],TEXT ; (null, text): the result is b
        .jcc(Cond::NotEqual, original) // jne original
        .bytes(&[0x8B, 0x54, 0x24, 0x04]) // mov edx,[esp+4]
        .bytes(&[0x89, 0x10]) // mov [eax],edx
        .bytes(&[0x8B, 0x54, 0x24, 0x08]) // mov edx,[esp+8]
        .bytes(&[0x89, 0x50, 0x04]) // mov [eax+4],edx
        .bytes(&[0xC3]) // ret
        .bind(a_number)
        .bytes(&[0x80, 0xFA, tags.number]) // cmp dl,NUMBER
        .jcc(Cond::NotEqual, a_other) // jne a_other
        .bytes(&[0x80, 0x7C, 0x24, 0x04, NULL]) // cmp byte [esp+4],0 ; (number, null): b becomes number 0
        .jcc(Cond::NotEqual, original) // jne original
        .bytes(&[0xC7, 0x44, 0x24, 0x04, tags.number, 0x00, 0x00, 0x00]) // mov dword [esp+4],NUMBER
        .bytes(&[0xC7, 0x44, 0x24, 0x08, 0x00, 0x00, 0x00, 0x00]) // mov dword [esp+8],0
        .jmp(original) // jmp original
        .bind(a_other)
        .bytes(&[0x80, 0xFA, tags.text]) // cmp dl,TEXT
        .jcc(Cond::NotEqual, original) // jne original
        .bytes(&[0x80, 0x7C, 0x24, 0x04, NULL]) // cmp byte [esp+4],0 ; (text, null): the result is a
        .jcc(Cond::NotEqual, original) // jne original
        .bytes(&[0x89, 0x10]) // mov [eax],edx
        .bytes(&[0x89, 0x48, 0x04]) // mov [eax+4],ecx
        .bytes(&[0xC3]) // ret
        .bind(original)
        .bytes(prologue) // the original's first instructions
        .jmp(f + PROLOGUE) // jmp back into the original
        .exec()
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let (first, second) = find(core, 0)?;
    let tags = tags(core, first, second)?;
    for f in [first, second] {
        core.untouched(&[(f, PROLOGUE)])?;
        core.jump(f, PROLOGUE, stub(f, core.bytes(f, PROLOGUE)?, tags)?)?;
    }
    Ok(core.report(&[first, second]))
}
