//! `outputleak`, Linux: `..()`'s output, input exits, and "bad input src" error go through stubs that release the args,
//! which are in the handler's own arguments.

use crate::base::code::{Cond, Stub};
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// `..()`'s output through 516.1688: mov ebp,[esp+count] ; test ebp,ebp ; je done ; cmp dword [esp+count],2 ;
/// jbe fewer ; mov eax,[esp+args] ; ... ; call output ; add esp,20h ; mov byte [edi],0 ; mov dword [edi+4],0 ; jmp
const OUTPUT: &str = "8B AC 24 ?? ?? ?? ?? 85 ED 74 ?? 83 BC 24 ?? ?? ?? ?? 02 76 ?? 8B 84 24 ?? ?? ?? ?? 8D 48 10 83 C0 08 \
                      83 EC 0C 8B 50 04 8B 00 6A 01 8B B4 24 ?? ?? ?? ?? FF 76 04 FF 36 FF 71 04 FF 31 E8 ?? ?? ?? ?? 83 \
                      C4 20 C6 07 00 C7 47 04 00 00 00 00 E9";
const OUTPUT_CALL: usize = 61;
/// `..()`'s input: mov ebx,[esp+count] ; test ebx,ebx ; je ; cmp dword [esp+count],1 ; je own ; mov eax,[esp+args]
const INPUT: &str = "8B 9C 24 ?? ?? ?? ?? 85 DB 0F 84 ?? ?? ?? ?? 83 BC 24 ?? ?? ?? ?? 01 0F 84 ?? ?? ?? ?? 8B 84 24 ?? ?? ?? \
                     ??";
const INPUT_COUNT: [usize; 2] = [3, 18];
const INPUT_ARGS: usize = 32;
/// After the source's type checks: cmp dl/bl,2Bh (a savefile) ; je savefile ; sub esp,0Ch, then the "bad input
/// src" error.
const BAD: &str = "80 ?? 2B 0F 84 ?? ?? ?? ?? 83 EC 0C";
/// The `cmp` and the `je` replaced by the stub.
const BAD_CMP: usize = 3;
const BAD_LEN: usize = 9;
/// The client and savefile reads' exits through 516.1688: push eax ; push edi ; call read ; add esp,0Ch ; jmp done
const READ_EXIT: &str = "50 57 E8 ?? ?? ?? ?? 83 C4 0C E9";
const READ_EXIT_AT: usize = 7;
const READ_EXITS_SPAN: usize = 0x200;

/// sub esp,8 ; push [esp+count+8] ; push [esp+args+0Ch] ; call release ; add esp,10h, at the handler's own stack
/// depth (16-byte aligned).
fn release(stub: &mut Stub, release: usize, count: u32, args: u32) -> &mut Stub {
    stub.bytes(&[0x83, 0xEC, 0x08])
        .bytes_u32(&[0xFF, 0xB4, 0x24], count.wrapping_add(8), &[])
        .bytes_u32(&[0xFF, 0xB4, 0x24], args.wrapping_add(0xC), &[])
        .call(release)
        .bytes(&[0x83, 0xC4, 0x10])
}

pub(super) fn install(core: &Image, rel: usize) -> Result<Vec<usize>> {
    let input = core.find_unique(INPUT, "..() input")?;
    let count = core.u32_at(input + INPUT_COUNT[0])?;
    ensure!(core.u32_at(input + INPUT_COUNT[1])? == count, "..() input: two arg counts");
    let args = core.u32_at(input + INPUT_ARGS)?;
    let bad = core.find_unique_in(input, 0x60, BAD, "..() input: bad source")?;
    let savefile = core.jcc_at(bad + BAD_CMP, Cond::Equal)?.target;
    let mut stub = Stub::new();
    stub.bytes(core.bytes(bad, BAD_CMP)?).jcc(Cond::Equal, savefile);
    release(&mut stub, rel, count, args).jmp(bad + BAD_LEN);
    let stub = stub.exec()?;
    core.untouched(&[(bad, BAD_LEN)])?;
    core.jump(bad, BAD_LEN, stub)?;
    let Some(output) = core.probe_all(OUTPUT)?.first() else {
        // 516.1689 releases after output and input itself.
        return Ok(vec![input]);
    };
    ensure!(
        core.u32_at(output + 3)? == count && core.u32_at(output + 24)? == args,
        "..() output: other args than input's"
    );
    let site = output + OUTPUT_CALL;
    let mut stub = Stub::new();
    stub.call(core.call_target(site)?).bytes(&[0x83, 0xC4, 0x20]); // add esp,20h
    release(&mut stub, rel, count, args).jmp(site + 8);
    let stub = stub.exec()?;
    core.untouched(&[(site, 8)])?;
    core.jump(site, 8, stub)?;
    let exits = core.find_in(input.saturating_sub(READ_EXITS_SPAN), 2 * READ_EXITS_SPAN, READ_EXIT)?;
    ensure!(exits.len() == 2, "..() input: expected a client and a savefile exit, found {}", exits.len());
    for &exit in exits.iter() {
        let at = exit + READ_EXIT_AT;
        let done = core.rel32(at + 4)?;
        let mut stub = Stub::new();
        stub.bytes(&[0x83, 0xC4, 0x0C]); // add esp,0Ch
        release(&mut stub, rel, count, args).jmp(done);
        let stub = stub.exec()?;
        core.untouched(&[(at, 8)])?;
        core.jump(at, 8, stub)?;
    }
    Ok(vec![input, output])
}
