//! The rules every patch body keeps to, so a closed-source body can't reach the operating system on its own: it
//! decodes as plain code that ends in a `jmp`, `ret`, `int3`, or `ud2`; its branches go into itself, back to the site,
//! to the start of one of BYOND's own functions (not denied ones, see `deny.txt`), or to the checked Direct3D helper
//! (`com.rs`); it calls other BYOND DLLs only through their existing import slots; it reads no import table; it has
//! no system calls, interrupts, port I/O, privileged instructions, segment tricks, or other indirect branches; it only
//! moves `esp` the ordinary ways; and every address in it is a declared fixup (its own functions' addresses are
//! instruction starts). Its constants are in non-executable data.
//!
//! These are static rules on the bytes, and a determined body can get around them: it can compute an address from
//! an allowed one (`[global + n]` reaching the import table) and jump there with `push` and `ret`, or overwrite a
//! return address on the stack. So they catch mistakes and make a deliberate escape stand out to anyone reading the
//! disassembly; they don't sandbox the code. A body may also misuse an allowed BYOND function or BYOND's data.

use std::collections::{BTreeSet, HashMap};

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind, Register,
};

use crate::com;
use crate::pe::{DIR_IAT, DIR_IMPORT, Pe, Sweep, u32_at};
use dmfast_format::manifest::{Class, Entry, Field, Fixup, Manifest};

/// Names a body may not call, one case-insensitive substring per line (`#` starts a comment).
const DENY: &str = include_str!("../deny.txt");
/// The DLLs a body may call through import slots.
const BYOND_DLLS: [&str; 3] = ["byondcore.dll", "byondwin.dll", "byondext.dll"];

fn denied(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    DENY.lines()
        .filter_map(|l| l.split('#').next()?.split_whitespace().next())
        .find(|d| name.contains(&d.to_ascii_lowercase()))
}

/// What a body does (one line each) and the rules it breaks.
pub struct Checked {
    pub report: Vec<String>,
    pub problems: Vec<String>,
}

/// Checks `e`'s body (`body`, its bytes as written in the patched DLL) against the stock DLL it was applied to.
pub fn check(stock: &Pe, sweep: &Sweep, m: &Manifest, e: &Entry, body: &[u8]) -> Checked {
    let mut v = Validator {
        stock,
        sweep,
        m,
        e,
        fixups: e.fixups.iter().map(|f| (f.field_rva, f)).collect(),
        claimed: BTreeSet::new(),
        problems: Vec::new(),
        calls: BTreeSet::new(),
        reads: BTreeSet::new(),
        body_refs: Vec::new(),
        d3d: false,
    };
    let mut starts = BTreeSet::new();
    let mut inner = Vec::new();
    let mut last = None;
    let mut info = InstructionInfoFactory::new();
    let mut dec = Decoder::with_ip(32, body, u64::from(stock.base + e.body), DecoderOptions::NONE);
    let mut ins = Instruction::default();
    while dec.can_decode() {
        dec.decode_out(&mut ins);
        let at = ins.ip32() - stock.base;
        if ins.is_invalid() {
            v.problem(at, "not an instruction");
            continue;
        }
        starts.insert(at);
        if let Some(to) = v.instruction(&ins, &dec, &mut info) {
            inner.push((at, to));
        }
        last = Some(ins);
    }
    let ends = |i: &Instruction| {
        matches!(i.flow_control(), FlowControl::UnconditionalBranch | FlowControl::Return | FlowControl::IndirectBranch)
            || matches!(i.mnemonic(), Mnemonic::Int3 | Mnemonic::Ud2)
    };
    if !last.is_some_and(|i| ends(&i)) {
        v.problem(e.body, "doesn't end in a jmp, ret, int3, or ud2");
    }
    if !starts.contains(&e.entry) {
        v.problem(e.entry, "the entry isn't an instruction of the body");
    }
    for (at, to) in inner {
        if !starts.contains(&to) {
            v.problem(at, &format!("branches to +{to:#x}, not an instruction of the body"));
        }
    }
    for (at, to) in std::mem::take(&mut v.body_refs) {
        if !starts.contains(&to) {
            v.problem(at, &format!("takes the address +{to:#x}, not an instruction of the body"));
        }
    }
    for f in &e.fixups {
        if !v.claimed.contains(&f.field_rva) {
            v.problem(f.field_rva, "fixup isn't an instruction's address field");
        }
        let at = (f.field_rva.wrapping_sub(e.body)) as usize;
        let want = match f.field {
            Field::Rel32 => f.target.wrapping_sub(f.field_rva + 4),
            Field::Abs32 => stock.base.wrapping_add(f.target),
        };
        if u32_at(body, at) != Some(want) {
            v.problem(f.field_rva, "field doesn't hold its fixup's address");
        }
    }
    let mut report = vec![format!("{} instructions", starts.len())];
    if !v.calls.is_empty() {
        report.push(format!("calls: {}", v.calls.iter().cloned().collect::<Vec<_>>().join(", ")));
    }
    if !v.reads.is_empty() {
        report.push(format!("data it uses: {}", v.reads.iter().cloned().collect::<Vec<_>>().join(", ")));
    }
    if v.d3d {
        report.push("calls Direct3D 9 methods (checked helper)".into());
    }
    Checked { report, problems: v.problems }
}

struct Validator<'a> {
    stock: &'a Pe<'a>,
    sweep: &'a Sweep,
    m: &'a Manifest,
    e: &'a Entry,
    fixups: HashMap<u32, &'a Fixup>,
    claimed: BTreeSet<u32>,
    problems: Vec<String>,
    /// Functions and imports it calls or jumps to.
    calls: BTreeSet<String>,
    /// Data it uses.
    reads: BTreeSet<String>,
    /// Where it takes the address of its own code, and that address (each must be an instruction start).
    body_refs: Vec<(u32, u32)>,
    /// Whether it calls the Direct3D helper.
    d3d: bool,
}

impl Validator<'_> {
    fn problem(&mut self, at: u32, what: &str) {
        self.problems.push(format!("{} +{:#x}: {what}", self.e.name, at));
    }

    /// The fixup of the 4-byte field at `field`, marked as used.
    fn fixup(&mut self, field: u32) -> Option<Fixup> {
        let f = (*self.fixups.get(&field)?).clone();
        self.claimed.insert(field);
        Some(f)
    }

    fn name_of(&self, rva: u32) -> String {
        match self.stock.exports_at(rva).next() {
            Some(n) => n.to_string(),
            None => format!("internal +{rva:#x}"),
        }
    }

    /// Checks one instruction; returns the target of a branch that must land on one of the body's instructions.
    fn instruction(&mut self, ins: &Instruction, dec: &Decoder, info: &mut InstructionInfoFactory) -> Option<u32> {
        let base = self.stock.base;
        let at = ins.ip32() - base;
        self.forbidden(ins, at, info);
        let offsets = dec.get_constant_offsets(ins);
        let indirect = matches!(ins.flow_control(), FlowControl::IndirectBranch | FlowControl::IndirectCall);
        if offsets.has_displacement() && offsets.displacement_size() == 4 {
            let field = at + offsets.displacement_offset() as u32;
            let absolute = ins.memory_base() == Register::None && ins.memory_index() == Register::None;
            match self.fixup(field) {
                Some(f) => self.address(&f, at, indirect && absolute),
                None if absolute => self.problem(at, "absolute address without a fixup"),
                None => {}
            }
        }
        if indirect {
            let via_import = ins.op0_kind() == OpKind::Memory
                && ins.memory_base() == Register::None
                && ins.memory_index() == Register::None
                && self
                    .fixups
                    .get(&(at + offsets.displacement_offset() as u32))
                    .is_some_and(|f| f.class == Class::Import);
            if !via_import {
                self.problem(at, "indirect branch (only `call/jmp dword ptr [import slot]`)");
            }
            return None;
        }
        if matches!(ins.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16) {
            let to = (ins.near_branch_target() as u32).wrapping_sub(base);
            let rel32 = ins.is_call_near() || ins.is_jmp_near() || ins.is_jcc_near();
            let Some(f) = rel32.then(|| self.fixup(at + ins.len() as u32 - 4)).flatten() else {
                return Some(to);
            };
            if f.field != Field::Rel32 || f.target != to {
                self.problem(at, "branch field doesn't match its fixup");
                return None;
            }
            return self.branch(&f, at);
        }
        if offsets.has_immediate() && offsets.immediate_size() == 4 {
            let field = at + offsets.immediate_offset() as u32;
            if let Some(f) = self.fixup(field) {
                self.address(&f, at, false);
            }
        }
        None
    }

    /// A rel32 branch's fixup target; `Some` for one inside the body.
    fn branch(&mut self, f: &Fixup, at: u32) -> Option<u32> {
        match f.class {
            Class::Body => return Some(f.target),
            Class::Resume if f.target == self.e.site + self.e.displace => {}
            Class::Original if f.target == self.e.orig => {}
            Class::Code => {
                let to = f.target;
                let function = self.stock.exports_at(to).next().is_some() || self.sweep.called(to);
                if !self.stock.is_code(to) || !self.sweep.starts_at(to) || !function {
                    self.problem(at, &format!("calls +{to:#x}, not the start of a function in the DLL's code"));
                } else if self.stock.bytes(to, 2) == Some(&[0xFF, 0x25]) {
                    self.problem(at, &format!("calls +{to:#x}, an import thunk"));
                } else if let Some(d) = self.stock.exports_at(to).find_map(denied) {
                    self.problem(at, &format!("calls {}, denied ({d})", self.name_of(to)));
                }
                let name = self.name_of(to);
                self.calls.insert(name);
            }
            Class::Com | Class::ComTail => {
                let entry = self.m.com.wrapping_add(if f.class == Class::Com { 0 } else { com::TAIL });
                if self.m.com == 0 || f.target != entry {
                    self.problem(at, "branch to the Direct3D helper that isn't one of its two entries");
                }
                self.d3d = true;
            }
            _ => self.problem(at, "branch fixup to something that isn't code"),
        }
        None
    }

    /// An `Abs32` field's fixup; `import_operand` when it's the slot of a `call/jmp dword ptr [slot]`.
    fn address(&mut self, f: &Fixup, at: u32, import_operand: bool) {
        if f.field != Field::Abs32 {
            self.problem(at, "address field with a rel32 fixup");
            return;
        }
        let to = f.target;
        match f.class {
            Class::Data if (self.m.data_rva..self.m.data_rva + self.m.data_len).contains(&to) => {
                self.reads.insert("its own data".into());
            }
            Class::Body if (self.e.body..self.e.body + self.e.body_len).contains(&to) => {
                self.body_refs.push((at, to));
            }
            // The audit prints the constants themselves.
            Class::Const if (self.e.consts..self.e.consts + self.e.consts_len).contains(&to) => {}
            Class::Global
                if self.stock.section_of(to).is_some_and(|s| !s.exec())
                    && !self.stock.in_dir(DIR_IAT, to)
                    && !self.stock.in_dir(DIR_IMPORT, to) =>
            {
                let s = self.stock.section_of(to).unwrap();
                let name = String::from_utf8_lossy(&s.name).trim_end_matches('\0').to_string();
                self.reads.insert(format!("{name} +{to:#x}"));
            }
            Class::Import if import_operand => match self.stock.import_at(to) {
                Some((dll, name)) if BYOND_DLLS.contains(&dll) => {
                    if let Some(d) = denied(name) {
                        self.problem(at, &format!("calls {dll}!{name}, denied ({d})"));
                    }
                    self.calls.insert(format!("{dll}!{name}"));
                }
                Some((dll, name)) => self.problem(at, &format!("calls {dll}!{name}: not a BYOND DLL")),
                None => self.problem(at, &format!("+{to:#x} isn't an import slot")),
            },
            _ => self.problem(
                at,
                "address that isn't its own data, constants, or code, BYOND's data, or an import slot operand",
            ),
        }
    }

    /// Instructions no body may use.
    fn forbidden(&mut self, ins: &Instruction, at: u32, info: &mut InstructionInfoFactory) {
        use Mnemonic::*;
        let m = ins.mnemonic();
        if ins.is_privileged() || (ins.flow_control() == FlowControl::Interrupt && m != Int3) {
            self.problem(at, &format!("{m:?}: interrupts, system calls, and privileged instructions are not allowed"));
        }
        if matches!(m, Syscall | Sysenter | Sysexit | Iret | Iretd | Retf | Lds | Les | Lfs | Lgs | Lss)
            || ins.is_call_far()
            || ins.is_jmp_far()
            || ins.is_call_far_indirect()
            || ins.is_jmp_far_indirect()
        {
            self.problem(at, &format!("{m:?}: far branches, system calls, and segment loads are not allowed"));
        }
        if matches!(ins.segment_prefix(), Register::FS | Register::GS)
            || (ins.op_kinds().any(|k| k == OpKind::Memory)
                && matches!(ins.memory_segment(), Register::FS | Register::GS))
        {
            self.problem(at, "fs/gs memory (the thread and process blocks) is not allowed");
        }
        if (0..ins.op_count()).any(|i| ins.op_kind(i) == OpKind::Register && ins.op_register(i).is_segment_register()) {
            self.problem(at, "segment registers are not allowed");
        }
        let writes_esp = info.info(ins).used_registers().iter().any(|r| {
            r.register() == Register::ESP
                && matches!(
                    r.access(),
                    OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite
                )
        });
        let stack_op = matches!(m, Push | Pop | Pushad | Popad | Pushfd | Popfd | Call | Ret | Leave)
            && !(m == Pop && ins.op0_kind() == OpKind::Register && ins.op0_register() == Register::ESP);
        let adjust = matches!(m, Add | Sub | And)
            && ins.op0_kind() == OpKind::Register
            && ins.op0_register() == Register::ESP
            && matches!(ins.op1_kind(), OpKind::Immediate32 | OpKind::Immediate8to32 | OpKind::Immediate8);
        // A frame's epilogue: `mov esp, ebp`.
        let frame = m == Mov
            && (ins.op0_kind(), ins.op1_kind()) == (OpKind::Register, OpKind::Register)
            && (ins.op0_register(), ins.op1_register()) == (Register::ESP, Register::EBP);
        if writes_esp && !stack_op && !adjust && !frame {
            self.problem(
                at,
                &format!(
                    "{m:?} changes esp (only push, pop, call, ret, leave, mov esp, ebp, and add/sub/and esp, imm)"
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_list_matches_substrings() {
        assert_eq!(denied("?Login@DungClient@@QAEXPBD0W4PwordCryptLevel@@@Z"), Some("login"));
        assert_eq!(denied("?HashFrames@DMIcon@@QAEXXZ"), None);
        assert!(
            DENY.lines().all(|l| l.starts_with('#') || l.split('#').next().unwrap().split_whitespace().count() == 1)
        );
    }
}
