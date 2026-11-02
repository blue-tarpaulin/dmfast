//! `tickinit` and `tick0`, Linux: where the tick keeps its state, and its usage and `/world/Tick` calls.

use std::arch::asm;

use super::{Avg, Tick, in_tick_flag};
use crate::base::code::{Foreign, live_u32, read};
use crate::base::image::Image;
use crate::base::{Result, ensure};

/// The world reset: sub esp,14h ; push 0 ; push x ; call ; add esp,10h ; call ; sub esp,0Ch ; push [y] ;
/// mov byte [z],0 ; call ; add esp,10h ; mov dword [w],0
pub(super) const RESET: &str = "83 EC 14 6A 00 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 E8 ?? ?? ?? ?? 83 EC 0C FF 35 \
                                ?? ?? ?? ?? C6 05 ?? ?? ?? ?? 00 E8 ?? ?? ?? ?? 83 C4 10 C7 05";
/// sub esp,14h ; push 0: 5 position-independent bytes.
pub(super) const RESET_ENTRY: usize = 5;

/// The tick usage and the call of `/world/Tick`, as the tick makes them.
pub(super) struct Calls {
    /// The usage's scale (f32).
    scale: usize,
    /// The event call, the event id, and the argument globals.
    event: usize,
    id: u32,
    args: [usize; 4],
}

impl Calls {
    /// The tick usage as the tick computes it: `(Ticks() - start) / (tick length * scale)`, the tick length at
    /// `[[world]+0Ch]+28h`.
    ///
    /// # Safety
    /// From the tick timer's callback.
    pub(super) unsafe fn usage(&self, t: &Tick) -> f32 {
        // SAFETY: the clock, world pointer, and scale were located at install.
        unsafe {
            let diff = t.ticks.get()().wrapping_sub(read::<u64>(t.start));
            let world = read::<usize>(t.world);
            let lag = read::<u32>(read::<usize>(world + 0xC) + 0x28);
            (diff as f64 / (f64::from(lag) * f64::from(read::<f32>(self.scale)))) as f32
        }
    }

    /// The call as the tick makes it: a hidden result pointer (popped by the callee) and 10 more arguments.
    ///
    /// # Safety
    /// From the tick timer's callback, as the tick calls it.
    pub(super) unsafe fn world_tick(&self) {
        // SAFETY: the event function and argument globals found at install; called as the tick does.
        unsafe {
            let mut ret = [0u32; 4];
            let [a0, a1, a2, a3] = self.args.map(|a| live_u32(a));
            // Read by the pushes below: the arguments, the id, the result pointer, and the function.
            let v = [a0, a1, a2, a3, self.id, ret.as_mut_ptr() as u32, self.event as u32];
            asm!(
                "mov edi, esp",
                "and esp, -16",
                "sub esp, 4",
                "push 0",
                "push 0",
                "push 0",
                "push 0",
                "push dword ptr [ecx + 12]",
                "push dword ptr [ecx + 8]",
                "push dword ptr [ecx + 16]",
                "push 0x16",
                "push dword ptr [ecx + 4]",
                "push dword ptr [ecx]",
                "push dword ptr [ecx + 20]",
                "call dword ptr [ecx + 24]",
                "mov esp, edi",
                in("ecx") v.as_ptr(),
                out("edi") _,
                clobber_abi("C"),
            );
        }
    }
}

/// The tick and the operand of its timer registration.
pub(super) fn locate(core: &Image) -> Result<(Tick, usize)> {
    // The tick: mov eax,[world] ; test eax,eax ; je ; push edi/esi/ebx ; sub esp,48h ; push 0 ; push x ;
    // call ; add esp,10h ; call Ticks ; mov ecx,[crash trace] ; movd xmm0,eax
    let f = core.find_unique(
        "A1 ?? ?? ?? ?? 85 C0 0F 84 ?? ?? ?? ?? 57 56 53 83 EC 48 6A 00 68 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 10 \
         E8 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 66 0F 6E C0",
        "tick function",
    )?;
    let world = core.ptr_at(f + 1)?;
    let ticks = core.rel32(f + 35)?;
    const SPAN: usize = 0x800;
    // movq [start],xmm0 ; test ecx,ecx ; jne
    let start = core.ptr_at(core.find_unique_in(f, SPAN, "66 0F D6 05 ?? ?? ?? ?? 85 C9 0F 85", "tick start")? + 4)?;
    // add [x],1 ; mov eax,[eax+0Ch] ; add [world.tick],1
    let counters =
        core.find_unique_in(f, SPAN, "83 05 ?? ?? ?? ?? 01 8B 40 0C 83 05 ?? ?? ?? ?? 01", "tick counters")?;
    let tick_count = core.ptr_at(counters + 12)?;
    let in_tick = in_tick_flag(core, f, SPAN)?;
    // cmp [dmb version],202h ; ja
    let version =
        core.ptr_at(core.find_unique_in(f, SPAN, "81 3D ?? ?? ?? ?? 02 02 00 00 0F 87", "dmb version test")? + 2)?;
    // lea eax,[esp+30h] (the result) ; sub esp,4 ; push 0 x4 ; push [a3] ; push [a2] ; push id ; push 16h ;
    // push [a1] ; push [a0] ; push eax ; call event ; add esp,2Ch
    let call = core.find_unique_in(
        f,
        SPAN,
        "8D 44 24 30 83 EC 04 6A 00 6A 00 6A 00 6A 00 FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? 68 ?? ?? ?? ?? 6A 16 \
         FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? 50 E8 ?? ?? ?? ?? 83 C4 2C",
        "/world/Tick call",
    )?;
    // world.cpu: fmul [usage scale] ; mov eax,ecx ; add eax,1 ; and eax,0Fh ; fdivp ; mov [index],al ; fstp/fld ;
    // fld [sum] ; fsub [samples + ecx*4] ; fadd ; fst [sum] ; fxch ; fstp [samples + ecx*4] ; fmul [scale] ;
    // fst [result] ; fld [map result] ; fld st(1) ; fsub st(1) ; fstp [result without the map]
    let c = core.find_unique_in(
        f,
        SPAN,
        "D8 0D ?? ?? ?? ?? 89 C8 83 C0 01 83 E0 0F DE F9 A2 ?? ?? ?? ?? D9 1C 24 D9 04 24 D9 05 ?? ?? ?? ?? \
         D8 24 8D ?? ?? ?? ?? D8 C1 D9 15 ?? ?? ?? ?? D9 C9 D9 1C 8D ?? ?? ?? ?? D8 0D ?? ?? ?? ?? D9 15 ?? ?? ?? ?? \
         D9 05 ?? ?? ?? ?? D9 C1 D8 E1 D9 1D ?? ?? ?? ??",
        "world.cpu average",
    )?;
    let cpu = Avg {
        index: core.ptr_at(c + 17)?,
        samples: core.ptr_at(c + 36)?,
        sum: core.ptr_at(c + 29)?,
        scale: core.ptr_at(c + 59)?,
    };
    ensure!(
        core.ptr_at(c + 44)? == cpu.sum && core.ptr_at(c + 53)? == cpu.samples,
        "world.cpu average is not one sum and ring"
    );
    // Before it, the usage: mov eax,[world] ; (Ticks() - start) ; mov eax,[eax+0Ch] ; ... mov edx,[eax+28h]
    // (the tick length) ; fild the difference (+2^64 if negative) ; movzx ecx,byte [index] ; fild the tick length
    let u = core.find_unique_in(
        f,
        SPAN,
        "A1 ?? ?? ?? ?? 66 0F 62 C1 66 0F FB 04 24 8B 40 0C 66 0F 7E 44 24 ?? 66 0F 73 D0 20 8B 74 24 ?? \
         66 0F 7E 44 24 ?? 8B 7C 24 ?? 8B 50 28 66 0F 6E C6 66 0F 6E FF 66 0F 62 C7 66 0F D6 44 24 ?? DF 6C 24 ?? \
         85 FF 79 06 D8 05 ?? ?? ?? ?? 66 0F 6E C2 DD 1C 24 DD 04 24 0F B6 0D ?? ?? ?? ?? 66 0F D6 44 24 ?? \
         DF 6C 24 ??",
        "world.cpu usage",
    )?;
    ensure!(
        u + 104 == c && core.ptr_at(u + 1)? == world && core.ptr_at(u + 90)? == cpu.index,
        "world.cpu's usage is not computed right before its average"
    );
    let map_out = core.ptr_at(c + 71)?;
    // world.map_cpu: fld [sum] ; fsub [samples + edx*4] ; fadd ; fst [sum] ; fxch ; fstp [samples + edx*4] ;
    // fmul [scale] ; xor edx,edx ; cmp al,10h ; cmovae eax,edx ; mov [index],al ; fstp [result]
    let m = core.find_unique_in(
        f,
        SPAN,
        "D9 05 ?? ?? ?? ?? D8 24 95 ?? ?? ?? ?? D8 C1 D9 15 ?? ?? ?? ?? D9 C9 D9 1C 95 ?? ?? ?? ?? D8 0D ?? ?? ?? ?? \
         31 D2 3C 10 0F 43 C2 A2 ?? ?? ?? ?? D9 1D ?? ?? ?? ??",
        "world.map_cpu average",
    )?;
    let map = Avg {
        index: core.ptr_at(m + 44)?,
        samples: core.ptr_at(m + 9)?,
        sum: core.ptr_at(m + 2)?,
        scale: core.ptr_at(m + 32)?,
    };
    ensure!(
        core.ptr_at(m + 50)? == map_out && core.ptr_at(m + 17)? == map.sum && core.ptr_at(m + 26)? == map.samples,
        "world.map_cpu average is not one sum and ring"
    );
    // mov [skip],bx ; sub esp,0Ch ; mov byte [in tick],0
    let skip =
        core.find_unique_in(f, SPAN, "66 89 1D ?? ?? ?? ?? 83 EC 0C C6 05 ?? ?? ?? ?? 00", "map_cpu skip counter")?;
    // The timer registration: push <tick> ... call.
    let site = super::registration(core, f)?;
    let calls = Calls {
        scale: core.ptr_at(c + 2)?,
        event: core.rel32(call + 48)?,
        id: core.u32_at(call + 28)?,
        args: [core.ptr_at(call + 42)?, core.ptr_at(call + 36)?, core.ptr_at(call + 23)?, core.ptr_at(call + 17)?],
    };
    let tick = Tick {
        original: Foreign::at(core.ptr_at(site)?),
        ticks: Foreign::at(ticks),
        start,
        tick_count,
        world,
        in_tick,
        version,
        calls,
        cpu,
        cpu_out: core.ptr_at(c + 65)?,
        cpu_net: core.ptr_at(c + 81)?,
        map,
        map_out,
        map_skip: core.ptr_at(skip + 3)?,
    };
    Ok((tick, site))
}
