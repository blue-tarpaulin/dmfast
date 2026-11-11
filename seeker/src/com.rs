//! The checked helper that lets a patch call Direct3D 9, and nothing else, through COM: the prelude's `comN` call
//! `dmf_com`, and a patch's own forwarders jump to `dmf_com_tail`. It goes once into a patched DLL's code section when
//! a patch there uses it, with its 12-byte cache in the data section; it finds its cache with `call $+5 ; pop`, so it
//! has no relocations, and the audit compares it byte for byte with [`build`] (assembled with iced, shortest forms).
//!
//! - `dmf_com` (stdcall), stack `[ret][slot][this][args..]`: drops the slot into `eax`, then is `dmf_com_tail`.
//! - `dmf_com_tail`, `eax` = slot, stack as the method expects (`[ret][this][args..]`): jumps to method `slot` of
//!   `this` only when `slot < 256` and the method is in d3d9.dll's code; else `ud2`. It changes `eax`, `ecx`,
//!   `edx`, and the flags. Where the vtable is isn't checked: overlays (a GPU vendor's capture tool, say) give
//!   Direct3D's objects copied vtables of their own.
//!
//! d3d9.dll is found on first use in the PEB's loaded-module list (by `BaseDllName`, case-insensitively) and its
//! base, code start, and code end are cached.

use iced_x86::code_asm::*;

/// The helper's length in bytes.
pub const LEN: u32 = 267;
/// Where `dmf_com_tail` starts in it (`dmf_com` is at 0).
pub const TAIL: u32 = 4;
/// Its cache's length in the data section.
pub const CACHE_LEN: u32 = 12;

/// The helper at `at`, with its cache at `cache` (RVAs; the cache is outside the code section, so the offset to it
/// takes a 32-bit immediate).
pub fn build(at: u32, cache: u32) -> Vec<u8> {
    assemble(at, cache).expect("the helper assembles")
}

fn assemble(at: u32, cache: u32) -> Result<Vec<u8>, IcedError> {
    let mut a = CodeAssembler::new(32)?;
    let (mut here, mut scan, mut next) = (a.create_label(), a.create_label(), a.create_label());
    let (mut ready, mut fail) = (a.create_label(), a.create_label());
    // dmf_com: the slot into eax, then the tail.
    a.pop(eax)?;
    a.xchg(dword_ptr(esp), eax)?;
    // dmf_com_tail: ecx = the cache.
    a.call(here)?;
    a.set_label(&mut here)?;
    a.pop(ecx)?;
    a.add(ecx, cache.wrapping_sub(at + TAIL + 5) as i32)?;
    a.cmp(dword_ptr(ecx), 0)?;
    a.jne(ready)?;
    // First use: find d3d9.dll in the PEB's loaded-module list.
    a.push(eax)?;
    a.push(ebx)?;
    a.push(esi)?;
    a.push(edi)?;
    a.mov(eax, dword_ptr(0x30).fs())?; // the PEB
    a.mov(eax, dword_ptr(eax + 0x0C))?; // Ldr
    a.lea(ebx, dword_ptr(eax + 0x0C))?; // InLoadOrderModuleList
    a.mov(esi, dword_ptr(ebx))?;
    a.set_label(&mut scan)?;
    a.cmp(esi, ebx)?;
    a.je(fail)?; // not loaded
    a.cmp(word_ptr(esi + 0x2C), 16)?; // BaseDllName.Length
    a.jne(next)?;
    a.mov(edi, dword_ptr(esi + 0x30))?; // BaseDllName.Buffer
    for (i, &c) in b"d3d9.dll".iter().enumerate() {
        a.movzx(eax, word_ptr(edi + 2 * i as i32))?;
        a.or(eax, 0x20)?; // lower case
        a.cmp(eax, i32::from(c))?;
        a.jne(next)?;
    }
    a.mov(eax, dword_ptr(esi + 0x18))?; // DllBase
    a.mov(edx, dword_ptr(eax + 0x3C))?; // e_lfanew
    a.add(edx, eax)?;
    a.mov(ebx, dword_ptr(edx + 0x2C))?; // BaseOfCode
    a.add(ebx, eax)?;
    a.mov(dword_ptr(ecx + 4), ebx)?; // code start
    a.add(ebx, dword_ptr(edx + 0x1C))?; // SizeOfCode
    a.mov(dword_ptr(ecx + 8), ebx)?; // code end
    a.mov(dword_ptr(ecx), eax)?; // base, last: it says the cache is filled
    a.pop(edi)?;
    a.pop(esi)?;
    a.pop(ebx)?;
    a.pop(eax)?;
    a.jmp(ready)?;
    a.set_label(&mut next)?;
    a.mov(esi, dword_ptr(esi))?; // Flink
    a.jmp(scan)?;
    // The call: a slot of `this`'s vtable that holds an address in d3d9.dll's code.
    a.set_label(&mut ready)?;
    a.cmp(eax, 256)?;
    a.jae(fail)?;
    a.mov(edx, dword_ptr(esp + 4))?; // this
    a.mov(edx, dword_ptr(edx))?; // vtable
    a.mov(edx, dword_ptr(edx + eax * 4))?; // the method
    a.cmp(edx, dword_ptr(ecx + 4))?;
    a.jb(fail)?;
    a.cmp(edx, dword_ptr(ecx + 8))?;
    a.jae(fail)?;
    a.jmp(edx)?;
    a.set_label(&mut fail)?;
    a.ud2()?;
    let code = a.assemble(u64::from(at))?;
    assert_eq!(code.len() as u32, LEN, "the helper's length (the cache offset must need 32 bits)");
    Ok(code)
}

#[cfg(test)]
mod tests {
    use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic, OpKind, Register};

    use super::{CACHE_LEN, LEN, TAIL, build};

    #[test]
    fn decodes_as_written() {
        let (base, at, cache) = (0x1000_0000u32, 0x50_0000, 0x51_2340);
        let h = build(at, cache);
        assert_eq!(h.len() as u32, LEN);
        let ip = u64::from(base + at);
        let ins: Vec<_> = Decoder::with_ip(32, &h, ip, DecoderOptions::NONE).into_iter().collect();
        assert!(ins.iter().all(|i| !i.is_invalid()));
        let starts: Vec<u64> = ins.iter().map(|i| i.ip()).collect();
        let mn: Vec<_> = ins.iter().map(|i| i.mnemonic()).collect();
        use Mnemonic::*;
        assert_eq!(mn[..6], [Pop, Xchg, Call, Pop, Add, Cmp]);
        assert_eq!(ins[2].ip(), ip + u64::from(TAIL));
        assert_eq!(ins[2].near_branch_target(), ins[3].ip(), "call $+5");
        // The cache: the popped address plus the immediate.
        assert_eq!((ins[4].op0_register(), ins[4].op1_kind()), (Register::ECX, OpKind::Immediate32));
        assert_eq!((ins[3].ip() as u32).wrapping_add(ins[4].immediate32()), base + cache);
        // Its reads and writes (all dwords through ecx) stay in it.
        for i in ins.iter().filter(|i| i.memory_base() == Register::ECX) {
            assert!(i.memory_displacement32() + 4 <= CACHE_LEN, "{i:?} at {:#x}", i.ip());
        }
        let fs = ins.iter().find(|i| i.memory_segment() == Register::FS).unwrap();
        assert_eq!(fs.memory_displacement32(), 0x30);
        let name: Vec<u8> = ins
            .iter()
            .filter(|i| i.mnemonic() == Cmp && i.op0_register() == Register::EAX)
            .filter(|i| matches!(i.op1_kind(), OpKind::Immediate8to32 | OpKind::Immediate32) && i.immediate(1) < 256)
            .map(|i| i.immediate(1) as u8)
            .collect();
        assert_eq!(name, b"d3d9.dll");
        // Every branch lands on one of its instructions; the only way out is `jmp edx`; it ends in `ud2`.
        for i in &ins {
            if matches!(i.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16) {
                assert!(starts.contains(&i.near_branch_target()), "{i:?} at {:#x}", i.ip());
            }
        }
        let out: Vec<_> = ins.iter().filter(|i| i.flow_control() == FlowControl::IndirectBranch).collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op0_register(), Register::EDX);
        assert_eq!(ins.last().unwrap().mnemonic(), Ud2);
        let pushes = mn.iter().filter(|&&m| m == Push).count();
        let pops = mn.iter().filter(|&&m| m == Pop).count();
        assert_eq!((pushes, pops), (4, 6), "4 saved registers; dmf_com's pop and call $+5's pop");
    }
}
