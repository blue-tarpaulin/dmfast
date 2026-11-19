//! `outputleak` (forum report 2992664): an `operator<<` overload that outputs with `..()` (`usr << sound` through
//! it) no longer keeps a reference to its args, so they (a `/sound`, say) are freed.
//!
//! `..()` in an overload with no parent runs the operator's built-in code on the args it was given, which `..()` holds
//! references to. Every other way out of it releases them; the one for `<<` returns after the output without doing so.
//! The fix releases them there, with the same call the other ways use, as BYOND 516.1689 does. `>>`'s built-in input
//! has the same omission, but there `..()` gets no further than "bad input src" on these builds (overloaded input
//! statements crash before calling the overload), so it is left alone.
//!
//! Safe: the output is done with the args when it returns, and they are released once.

use crate::base::Result;
use crate::base::code::Stub;
#[cfg(target_os = "linux")]
use crate::base::ensure;
use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::image::Image;

pub static OUTPUTLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "outputleak",
    "<< overloads calling ..() release their args",
    Tier::Safe,
    install!(windows: install, linux: install),
)
.since(516_1665)
.fixed_in(516_1689, "https://www.byond.com/forum/post/2992664");

/// `..()`'s output: test ebx,ebx (the arg count) ; je done ; cmp ebx,2 ; jbe fewer ; (the target and the value from
/// the args at edi, or the proc's own args) ... ; push 1 ; push [edi+4] ; push [edi] ; push edx ; push ecx ;
/// push eax ; push esi ; call output ; add esp,1Ch (then the null result)
#[cfg(windows)]
const OUTPUT: &str = "85 DB 74 ?? 83 FB 02 76 ?? 8B 4F 10 8B 57 14 8B 77 08 8B 47 0C EB ?? 8B 0D ?? ?? ?? ?? 8B 15 ?? ?? ?? \
                      ?? 83 FB 01 76 ?? 8B 77 08 8B 47 0C EB ?? 8B 46 30 8B 70 08 8B 40 0C 6A 01 FF 77 04 FF 37 52 51 50 \
                      56 E8 ?? ?? ?? ?? 83 C4 1C";
#[cfg(windows)]
const CALL: usize = 68;
/// The other ways out release the args: push ebx ; push edi ; call release ; add esp,8 ; cmp byte [ebp+..],0 ; je
#[cfg(windows)]
const RELEASE: &str = "53 57 E8 ?? ?? ?? ?? 83 C4 08 80 7D ?? 00 74";
#[cfg(windows)]
const RELEASE_CALL: usize = 2;
#[cfg(windows)]
const RELEASE_SPAN: (isize, usize) = (0, 0x200);
/// The same on Linux, with the args and their count in the caller's arguments: mov ebp,[esp+count] ; test ebp,ebp ;
/// je done ; cmp dword [esp+count],2 ; jbe fewer ; mov eax,[esp+args] ; ... ; call output ; add esp,20h ;
/// mov byte [edi],0 ; mov dword [edi+4],0 (the null result) ; jmp
#[cfg(target_os = "linux")]
const OUTPUT: &str = "8B AC 24 ?? ?? ?? ?? 85 ED 74 ?? 83 BC 24 ?? ?? ?? ?? 02 76 ?? 8B 84 24 ?? ?? ?? ?? 8D 48 10 83 C0 08 \
                      83 EC 0C 8B 50 04 8B 00 6A 01 8B B4 24 ?? ?? ?? ?? FF 76 04 FF 36 FF 71 04 FF 31 E8 ?? ?? ?? ?? 83 \
                      C4 20 C6 07 00 C7 47 04 00 00 00 00 E9";
#[cfg(target_os = "linux")]
const CALL: usize = 61;
/// The count's offset (read twice) and the args'.
#[cfg(target_os = "linux")]
const COUNT: [usize; 2] = [3, 14];
#[cfg(target_os = "linux")]
const ARGS: usize = 24;
/// test bl,2 ; je ; cmp byte [esp+..],0 ; je ; sub esp,8 ; push [esp+count+8] ; push [esp+args+0Ch] ; call release ;
/// add esp,10h ; cmp byte [esp+..],0
#[cfg(target_os = "linux")]
const RELEASE: &str = "F6 C3 02 74 ?? 80 7C 24 ?? 00 74 ?? 83 EC 08 FF B4 24 ?? ?? ?? ?? FF B4 24 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 \
                       C4 10 80 7C 24 ?? 00";
/// Its pushes, their offsets, and the call.
#[cfg(target_os = "linux")]
const RELEASE_PUSHES: usize = 12;
#[cfg(target_os = "linux")]
const RELEASE_COUNT: usize = 18;
#[cfg(target_os = "linux")]
const RELEASE_ARGS: usize = 25;
#[cfg(target_os = "linux")]
const RELEASE_CALL: usize = 29;
#[cfg(target_os = "linux")]
const RELEASE_SPAN: (isize, usize) = (-0x400, 0x400);
/// `call output ; add esp,n`, replaced by a jump to the stub.
const CALL_LEN: usize = 8;

/// release(args, count) with the args at edi and their count in ebx.
#[cfg(windows)]
fn release(core: &Image, _: usize, at: usize, stub: &mut Stub) -> Result<()> {
    stub.bytes(&[0x53, 0x57]) // push ebx ; push edi
        .call(core.call_target(at + RELEASE_CALL)?)
        .bytes(&[0x83, 0xC4, 0x08]); // add esp,8
    Ok(())
}

/// release(args, count) with them in the caller's arguments, as the other way out pushes them.
#[cfg(target_os = "linux")]
fn release(core: &Image, output: usize, at: usize, stub: &mut Stub) -> Result<()> {
    let (count, args) = (core.u32_at(output + COUNT[0])?, core.u32_at(output + ARGS)?);
    ensure!(core.u32_at(output + COUNT[1])? == count, "..() output: two arg counts");
    ensure!(
        core.u32_at(at + RELEASE_COUNT)? == count.wrapping_add(8)
            && core.u32_at(at + RELEASE_ARGS)? == args.wrapping_add(0xC),
        "..() release: other args"
    );
    stub.bytes(core.bytes(at + RELEASE_PUSHES, RELEASE_CALL - RELEASE_PUSHES)?) // sub esp,8 ; push ; push
        .call(core.call_target(at + RELEASE_CALL)?)
        .bytes(&[0x83, 0xC4, 0x10]); // add esp,10h
    Ok(())
}

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let output = core.find_unique(OUTPUT, "..() output")?;
    let (back, span) = RELEASE_SPAN;
    let at = core.find_unique_in(output.wrapping_add_signed(back), span, RELEASE, "..() release")?;
    let site = output + CALL;
    let restore = core.bytes(site + 5, CALL_LEN - 5)?; // add esp,n
    let mut stub = Stub::new();
    stub.call(core.call_target(site)?).bytes(restore);
    release(core, output, at, &mut stub)?;
    let stub = stub.jmp(site + CALL_LEN).exec()?;
    core.untouched(&[(site, CALL_LEN)])?;
    core.jump(site, CALL_LEN, stub)?;
    Ok(core.report(&[output]))
}
