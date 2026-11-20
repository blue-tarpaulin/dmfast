//! `outputleak`, Windows: `..()`'s output and input calls go through stubs that release the args (in edi, their
//! count in ebx; on 516.1689 in ebx and a frame slot).

use crate::base::code::{Cond, Stub};
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// `..()`'s output: test ebx,ebx (the arg count) ; je done ; cmp ebx,2 ; jbe fewer ; (the target and the value from
/// the args at edi, or the proc's own args) ... ; push 1 ; push [edi+4] ; push [edi] ; push edx ; push ecx ;
/// push eax ; push esi ; call output ; add esp,1Ch (then the null result)
const OUTPUT: &str = "85 DB 74 ?? 83 FB 02 76 ?? 8B 4F 10 8B 57 14 8B 77 08 8B 47 0C EB ?? 8B 0D ?? ?? ?? ?? 8B 15 ?? ?? ?? \
                      ?? 83 FB 01 76 ?? 8B 77 08 8B 47 0C EB ?? 8B 46 30 8B 70 08 8B 40 0C 6A 01 FF 77 04 FF 37 52 51 50 \
                      56 E8 ?? ?? ?? ?? 83 C4 1C";
const OUTPUT_CALL: usize = 68;
/// `..()`'s input through 516.1688: push 0AFh ; call (the operator's id) ; ... ; test ebx,ebx ; je ; cmp ebx,1 ;
/// jbe own ; mov eax,[edi+0Ch] ; mov ecx,[edi+8] (the source) ; push eax ; push ecx ; call input ; add esp,8
const INPUT: &str = "68 AF 00 00 00 E8 ?? ?? ?? ?? 8B 4D ?? 83 C4 04 39 41 04 75 ?? 83 7E 2C 01 76 ?? 85 DB 74 ?? 83 FB 01 \
                     76 ?? 8B 47 0C 8B 4F 08 50 51 E8 ?? ?? ?? ?? 83 C4 08";
const INPUT_CALL: usize = 44;
/// Its other input call, with the proc's own src as the source: mov eax,[esi+30h] ; mov ecx,[eax+8] ;
/// mov eax,[eax+0Ch] ; push eax ; push ecx ; call input ; add esp,8
const INPUT_OWN: &str = "8B 46 30 8B 48 08 8B 40 0C 50 51 E8 ?? ?? ?? ?? 83 C4 08";
const INPUT_OWN_CALL: usize = 11;
/// 516.1689's single input call, which releases the args after: push eax ; push ecx ; call input ;
/// push [ebp+count] ; mov esi,eax ; mov edi,edx ; push ebx (the args) ; call release
const INPUT_1689: &str = "50 51 E8 ?? ?? ?? ?? FF 75 ?? 8B F0 8B FA 53 E8";
const INPUT_1689_CALL: usize = 2;
const INPUT_1689_COUNT: usize = 7;
const INPUT_1689_RELEASE: usize = 15;

/// The value types `>>`'s built-in input reads from: a client and a savefile.
const CLIENT: u8 = 0x23;
const SAVEFILE: u8 = 0x2B;

/// push ebx ; push edi: release(args, count) with the args at edi and their count in ebx.
const PUSH_ARGS: &[u8] = &[0x53, 0x57];

/// The stub for an input call at `site` (`push source ; call input`, the source's type at [esp]): a source that
/// isn't a client or a savefile gets "bad input src", so the args (pushed by `push`) are released first. With
/// `after`, the args are also released after the input, which returns in eax:edx; the stub takes the call and the
/// `add esp,8` after it.
fn input(core: &Image, site: usize, release: usize, push: &[u8], after: bool) -> Result<()> {
    let mut stub = Stub::new();
    let read = stub.label();
    stub.bytes(&[0x80, 0x3C, 0x24, CLIENT]) // cmp byte [esp],client
        .jcc8(Cond::Equal, read)
        .bytes(&[0x80, 0x3C, 0x24, SAVEFILE]) // cmp byte [esp],savefile
        .jcc8(Cond::Equal, read)
        .bytes(push)
        .call(release)
        .bytes(&[0x83, 0xC4, 0x08]) // add esp,8
        .bind(read)
        .call(core.call_target(site)?);
    let len = if after {
        stub.bytes(&[0x83, 0xC4, 0x08, 0x50, 0x52]) // add esp,8 ; push eax ; push edx
            .bytes(push)
            .call(release)
            .bytes(&[0x83, 0xC4, 0x08, 0x5A, 0x58]); // add esp,8 ; pop edx ; pop eax
        8
    } else {
        5
    };
    let stub = stub.jmp(site + len).exec()?;
    core.untouched(&[(site, len)])?;
    core.jump(site, len, stub)
}

pub(super) fn install(core: &Image, release: usize) -> Result<Vec<usize>> {
    let Some(output) = core.probe_all(OUTPUT)?.first() else {
        // 516.1689 releases after output and input itself, but not before "bad input src".
        let at = core.find_unique(INPUT_1689, "..() input")?;
        ensure!(core.call_target(at + INPUT_1689_RELEASE)? == release, "..() input: not the release");
        let push = [core.bytes(at + INPUT_1689_COUNT, 3)?, &[0x53]].concat(); // push [ebp+count] ; push ebx
        input(core, at + INPUT_1689_CALL, release, &push, false)?;
        return Ok(vec![at]);
    };
    let site = output + OUTPUT_CALL;
    let stub = Stub::new()
        .call(core.call_target(site)?)
        .bytes(&[0x83, 0xC4, 0x1C]) // add esp,1Ch
        .bytes(PUSH_ARGS)
        .call(release)
        .bytes(&[0x83, 0xC4, 0x08]) // add esp,8
        .jmp(site + 8)
        .exec()?;
    core.untouched(&[(site, 8)])?;
    core.jump(site, 8, stub)?;
    let at = core.find_unique(INPUT, "..() input")?;
    let own = core.find_unique_in(at, 0x60, INPUT_OWN, "..() input of its own src")?;
    ensure!(
        core.call_target(at + INPUT_CALL)? == core.call_target(own + INPUT_OWN_CALL)?,
        "..() input: two input functions"
    );
    input(core, at + INPUT_CALL, release, PUSH_ARGS, true)?;
    input(core, own + INPUT_OWN_CALL, release, PUSH_ARGS, true)?;
    Ok(vec![output, at])
}
