//! The gate each rewritten site jumps to: it goes to the patch's body only when the running program is
//! dreamseeker.exe (its PE header's `TimeDateStamp` and `SizeOfImage`, read through the PEB), and to the site's
//! original instructions otherwise. Every register and flag is as at the site either way.

use dmfast_format::manifest::ExeId;

/// The gate's length in bytes.
pub const LEN: u32 = 51;
/// Where the `jmp` to the body and the one to the original instructions end.
const BODY_JMP_END: u32 = 43;

/// The gate at `at` (an RVA, as are `body`, the patch's entry, and `orig`).
pub fn build(exe: ExeId, at: u32, body: u32, orig: u32) -> Vec<u8> {
    let rel = |to: u32, end: u32| to.wrapping_sub(at + end).to_le_bytes();
    let mut g = Vec::with_capacity(LEN as usize);
    g.extend_from_slice(&[0x9C, 0x50, 0x51]); // pushfd ; push eax ; push ecx
    g.extend_from_slice(&[0x64, 0xA1, 0x30, 0, 0, 0]); // mov eax, fs:[30h] (the PEB)
    g.extend_from_slice(&[0x8B, 0x40, 0x08]); // mov eax, [eax+8] (the program's image base)
    g.extend_from_slice(&[0x8B, 0x48, 0x3C]); // mov ecx, [eax+3Ch] (e_lfanew)
    g.extend_from_slice(&[0x81, 0x7C, 0x08, 0x08]); // cmp dword ptr [eax+ecx+8], timestamp
    g.extend_from_slice(&exe.timestamp.to_le_bytes());
    g.extend_from_slice(&[0x75, 18]); // jne closed
    g.extend_from_slice(&[0x81, 0x7C, 0x08, 0x50]); // cmp dword ptr [eax+ecx+50h], image size
    g.extend_from_slice(&exe.image_size.to_le_bytes());
    g.extend_from_slice(&[0x75, 8]); // jne closed
    g.extend_from_slice(&[0x59, 0x58, 0x9D, 0xE9]); // pop ecx ; pop eax ; popfd ; jmp body
    g.extend_from_slice(&rel(body, BODY_JMP_END));
    g.extend_from_slice(&[0x59, 0x58, 0x9D, 0xE9]); // closed: pop ecx ; pop eax ; popfd ; jmp orig
    g.extend_from_slice(&rel(orig, LEN));
    debug_assert_eq!(g.len() as u32, LEN);
    g
}

#[cfg(test)]
mod tests {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};

    use super::*;

    #[test]
    fn decodes_as_written() {
        let exe = ExeId { timestamp: 0x1122_3344, image_size: 0x000D_C000 };
        let (base, at, body, orig) = (0x1000_0000u32, 0x50_0000, 0x50_0100, 0x50_0080);
        let g = build(exe, at, body, orig);
        assert_eq!(g.len() as u32, LEN);
        let ins: Vec<_> = Decoder::with_ip(32, &g, u64::from(base + at), DecoderOptions::NONE).into_iter().collect();
        let mn: Vec<_> = ins.iter().map(|i| i.mnemonic()).collect();
        use Mnemonic::*;
        assert_eq!(
            mn,
            [Pushfd, Push, Push, Mov, Mov, Mov, Cmp, Jne, Cmp, Jne, Pop, Pop, Popfd, Jmp, Pop, Pop, Popfd, Jmp]
        );
        assert_eq!((ins[3].memory_segment(), ins[3].memory_displacement32()), (Register::FS, 0x30));
        for (i, field, value) in [(6, 8, exe.timestamp), (8, 0x50, exe.image_size)] {
            let c = &ins[i];
            assert_eq!(
                (c.memory_base(), c.memory_index(), c.memory_displacement32()),
                (Register::EAX, Register::ECX, field)
            );
            assert_eq!((c.op1_kind(), c.immediate32()), (OpKind::Immediate32, value));
        }
        let closed = u64::from(base + at + BODY_JMP_END);
        assert_eq!((ins[7].near_branch_target(), ins[9].near_branch_target()), (closed, closed));
        assert_eq!(ins[14].ip(), closed);
        assert_eq!(ins[13].near_branch_target(), u64::from(base + body));
        assert_eq!(ins[17].near_branch_target(), u64::from(base + orig));
    }
}
