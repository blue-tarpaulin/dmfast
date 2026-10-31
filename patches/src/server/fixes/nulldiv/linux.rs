//! `nulldiv`, Linux: the `/` handler's null case jumps back to the interpreter loop through a stub that checks the
//! right operand first. A number goes to the number case (its zero check, then the divide in place at `ebx`), null
//! to "Division by zero".

use super::{NULL, NUMBER};
use crate::base::code::{branch, exec_code};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// The number case: `cmp byte [ebp+right type],2Ah ; jne other ; fldz ; fld [right value] ; fucompi st(1) ;
/// fstp st(0) ; jp div ; je error ; div: fld [right value] ; fdivr [ebx+4] ; fstp [ebx+4] ; jmp loop`.
const NUMBER_CASE: &str = "80 BD ?? ?? ?? ?? 2A 0F 85 ?? ?? ?? ?? D9 EE D9 85 ?? ?? ?? ?? DF E9 DD D8 7A 06 \
                           0F 84 ?? ?? ?? ?? D9 85 ?? ?? ?? ?? D8 7B 04 D9 5B 04 E9";
/// Its `je error`.
const ERROR_AT: usize = 27;
/// The null case: `test al,al ; jne bad ; sub esp,8 ; lea edi,[ebp-x] ; push 0 ; push edi ; call number0 ;
/// movq xmm0,[ebp-x] ; mov esi,[state] ; add esp,0Ch ; movq [ebx],xmm0 ; jmp loop`. Every arithmetic handler has
/// one: only the `/` handler's is relied on.
const NULL_CASE: &str = "84 C0 0F 85 ?? ?? ?? ?? 83 EC 08 8D BD ?? ?? ?? ?? 6A 00 57 E8 ?? ?? ?? ?? F3 0F 7E 85 ?? ?? \
                         ?? ?? 8B 35 ?? ?? ?? ?? 83 C4 0C 66 0F D6 03 E9 ?? ?? ?? ??";
/// Its `jmp loop`.
const LOOP_AT: usize = 46;
/// The dispatch on the left operand's type: `cmp al,2Ah ; je number ; ja other ; cmp al,0Ch ; je vector ;
/// cmp al,21h ; jne null`. Only the `/` handler's leads to its number case; the others are candidates, not relied
/// on.
const DISPATCH: &str = "3C 2A 0F 84 ?? ?? ?? ?? 0F 87 ?? ?? ?? ?? 3C 0C 0F 84 ?? ?? ?? ?? 3C 21 0F 85 ?? ?? ?? ??";

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let num = core.find_unique(NUMBER_CASE, "number / x case")?;
    let right_type = core.array::<4>(num + 2)?;
    let error = core.rel32(num + ERROR_AT + 2)?;
    let dispatch: Vec<usize> =
        core.probe_all(DISPATCH)?.into_iter().filter(|&d| core.rel32(d + 4).is_ok_and(|t| t == num)).collect();
    let [d] = dispatch[..] else { bail!("/ dispatch: expected 1 match, found {}", dispatch.len()) };
    core.rely(d, DISPATCH.split_whitespace().count())?;
    let null = core.rel32(d + 26)?;
    if !core.is_at(null, NULL_CASE)? {
        bail!("null / x case: not where the dispatch leads");
    }
    let jmp = null + LOOP_AT;
    let loop_head = core.rel32(jmp + 1)?;
    let stub = exec_code(31, |s| {
        let mut v = Vec::new();
        v.extend([0x80, 0xBD]);
        v.extend(right_type);
        v.push(NUMBER); // cmp byte [ebp+right type],2Ah
        v.extend([0x0F, 0x84]);
        v.extend((num.wrapping_sub(s + v.len() + 4) as u32).to_le_bytes()); // je number case
        v.extend([0x80, 0xBD]);
        v.extend(right_type);
        v.push(NULL); // cmp byte [ebp+right type],0
        v.extend([0x0F, 0x84]);
        v.extend((error.wrapping_sub(s + v.len() + 4) as u32).to_le_bytes()); // je error
        v.extend(branch(0xE9, s + v.len(), loop_head, 5)); // jmp loop (stock)
        v
    })?;
    core.jump(jmp, 5, stub)?;
    Ok(format!("at +{:#x}", null - core.base))
}
