//! `tickinit` and `tick0`, Windows: where the tick keeps its state, and its usage and `/world/Tick` calls.

use super::{Avg, Tick, in_tick_flag};
use crate::base::code::{Foreign, hx, live_u32};
use crate::base::image::Image;
use crate::base::{Result, ensure};

type UsageFn = unsafe extern "C" fn() -> f32;
type EventFn = unsafe extern "C" fn(u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) -> u32;

/// The world reset: push 0 ; mov ecx,<world string> ; call ; call ; push [x] ; mov byte [y],0 ; call [free]
pub(super) const RESET: &str =
    "6A 00 B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? E8 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? C6 05 ?? ?? ?? ?? 00 FF 15 ?? ?? ?? ??";
/// push 0 ; mov ecx,imm32: 7 position-independent bytes.
pub(super) const RESET_ENTRY: usize = 7;

/// The tick usage and the call of `/world/Tick`, as the tick makes them.
pub(super) struct Calls {
    /// BYOND's tick usage function: `(Ticks() - start) / tick length`.
    usage: Foreign<UsageFn>,
    /// The event call, the event id, and the argument globals.
    event: Foreign<EventFn>,
    id: u32,
    args: [usize; 4],
}

impl Calls {
    /// The tick's usage so far.
    ///
    /// # Safety
    /// From the tick timer's callback.
    pub(super) unsafe fn usage(&self, _: &Tick) -> f32 {
        // SAFETY: the usage function found at install.
        unsafe { self.usage.get()() }
    }

    /// Calls `/world/Tick` with the tick's arguments (cdecl).
    ///
    /// # Safety
    /// From the tick timer's callback, as the tick calls it.
    pub(super) unsafe fn world_tick(&self) {
        // SAFETY: the event function and argument globals found at install.
        unsafe {
            let a = |i: usize| live_u32(self.args[i]);
            self.event.get()(a(0), a(1), 0x16, self.id, a(2), a(3), 0, 0, 0, 0);
        }
    }
}

/// The tick and the operand of its timer registration.
pub(super) fn locate(core: &Image) -> Result<(Tick, usize)> {
    // The tick: push ebp ; mov ebp,esp ; sub esp,10h ; cmp [world],0 ; jne ; xor eax,eax ; mov esp,ebp ;
    // pop ebp ; ret ; push ebx/esi/edi ; call Now ; mov [now],eax ; mov [now+4],edx ; call Ticks ;
    // cmp [crash trace],0 ; mov ebx,eax ; mov eax,edx ; mov ecx,ebx ; mov esi,eax ; mov [ebp-4],eax ;
    // mov [start],ecx ; mov [start+4],esi
    let f = core.find_unique(
        "55 8B EC 83 EC 10 83 3D ?? ?? ?? ?? 00 75 06 33 C0 8B E5 5D C3 53 56 57 E8 ?? ?? ?? ?? A3 ?? ?? ?? ?? \
         89 15 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 3D ?? ?? ?? ?? 00 8B D8 8B C2 8B CB 8B F0 89 45 FC 89 0D ?? ?? ?? ?? \
         89 35 ?? ?? ?? ??",
        "tick function",
    )?;
    let ticks = core.rel32(f + 41)?;
    let start = core.ptr_at(f + 65)?;
    ensure!(core.ptr_at(f + 71)? == start + 4, "tick start is not a 64-bit global");
    const SPAN: usize = 0x600;
    // The tick counters: inc [ticks] ; inc [world.tick]
    let counters = core.find_unique_in(f, SPAN, "FF 05 ?? ?? ?? ?? FF 05 ?? ?? ?? ??", "tick counters")?;
    let tick_count = core.ptr_at(counters + 8)?;
    let in_tick = in_tick_flag(core, f, SPAN)?;
    // cmp [dmb version],203h ; jb ; push 0 x4 ; push [a3] ; push [a2] ; push id ; push 16h ; push [a1] ;
    // push [a0] ; call event ; add esp,28h
    let call = core.find_unique_in(
        f,
        SPAN,
        "81 3D ?? ?? ?? ?? 03 02 00 00 72 ?? 6A 00 6A 00 6A 00 6A 00 FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? 68 \
         ?? ?? ?? ?? 6A 16 FF 35 ?? ?? ?? ?? FF 35 ?? ?? ?? ?? E8 ?? ?? ?? ?? 83 C4 28",
        "/world/Tick call",
    )?;
    // world.map_cpu: mov cl,[index] ; mov eax,[eax+0Ch] ; mov eax,[eax+28h] ... movzx eax,cl ; inc cl ;
    // cmp cl,10h ... movss xmm1,[sum] ; subss xmm1,[samples + eax*4] ... movss [samples + eax*4],xmm0 ;
    // sbb al,al ; and al,cl ; mov [index],al (a ring of 16) ... mulss xmm1,[scale] ; movss [result],xmm1
    let m = core.find_unique_in(
        f,
        SPAN,
        "8A 0D ?? ?? ?? ?? 8B 40 0C 8B 40 28 66 0F 6E C8 F3 0F E6 C9 C1 E8 1F F2 0F 58 0C C5 ?? ?? ?? ?? 0F B6 C1 \
         FE C1 80 F9 10 F2 0F 59 0D ?? ?? ?? ?? F2 0F 5E C1 F3 0F 10 0D ?? ?? ?? ?? F3 0F 5C 0C 85 ?? ?? ?? ?? \
         F2 0F 5A C0 F3 0F 58 C8 F3 0F 11 04 85 ?? ?? ?? ?? 1A C0 22 C1 A2 ?? ?? ?? ??",
        "map_cpu average",
    )?;
    let m3 = core.find_unique_in(f, SPAN, "F3 0F 59 0D ?? ?? ?? ?? F3 0F 11 0D ?? ?? ?? ?? E8", "map_cpu result")?;
    let map = Avg {
        index: core.ptr_at(m + 2)?,
        samples: core.ptr_at(m + 65)?,
        sum: core.ptr_at(m + 56)?,
        scale: core.ptr_at(m3 + 4)?,
    };
    ensure!(
        core.ptr_at(m + 91)? == map.index && core.ptr_at(m + 82)? == map.samples,
        "world.map_cpu average is not one index and ring"
    );
    let map_out = core.ptr_at(m3 + 12)?;
    // world.cpu: mov cl,[index] ; movss xmm2,[sum] ... movzx eax,cl ; inc cl ; and cl,0Fh (a ring of 16) ;
    // test edx,edx ; mov [index],cl ... subss xmm2,[samples + eax*4] ... movss xmm1,[map result] ;
    // addss xmm2,xmm0 ; movss [samples + eax*4],xmm0 ; movss [sum],xmm2 ; mulss xmm2,[scale] ; comiss ;
    // movss [result],xmm2 ; subss xmm0,xmm1 ; movss [result - map share],xmm0
    let c1 = core.find_unique_in(
        f,
        SPAN,
        "8A 0D ?? ?? ?? ?? F3 0F 10 15 ?? ?? ?? ?? F2 0F 58 04 F5 ?? ?? ?? ?? DD 5D ?? F2 0F 2C 45 ?? F2 0F 59 05 \
         ?? ?? ?? ?? 2B D0 0F B6 C1 FE C1 80 E1 0F 85 D2 88 0D ?? ?? ?? ??",
        "world.cpu index",
    )?;
    let c2 = core.find_unique_in(f, SPAN, "F3 0F 5C 14 85 ?? ?? ?? ?? 66 0F 5A C1", "world.cpu samples")?;
    let c3 = core.find_unique_in(
        f,
        SPAN,
        "F3 0F 10 0D ?? ?? ?? ?? F3 0F 58 D0 F3 0F 11 04 85 ?? ?? ?? ?? F3 0F 11 15 ?? ?? ?? ?? F3 0F 59 15 \
         ?? ?? ?? ?? 0F 2F 15 ?? ?? ?? ?? 0F 28 C2 F3 0F 11 15 ?? ?? ?? ?? F3 0F 5C C1 F3 0F 11 05 ?? ?? ?? ??",
        "world.cpu result",
    )?;
    let cpu = Avg {
        index: core.ptr_at(c1 + 2)?,
        samples: core.ptr_at(c2 + 5)?,
        sum: core.ptr_at(c3 + 25)?,
        scale: core.ptr_at(c3 + 33)?,
    };
    ensure!(
        core.ptr_at(c1 + 10)? == cpu.sum && core.ptr_at(c1 + 53)? == cpu.index && core.ptr_at(c3 + 17)? == cpu.samples,
        "world.cpu average is not one sum, index and ring"
    );
    ensure!(core.ptr_at(c3 + 4)? == map_out, "world.cpu's map share is not world.map_cpu's result");
    // mov cx,[skip] ; mov edi,0FFFFh ; mov si,cx ; add cx,di ; mov [skip],cx
    let sk = core.find_unique_in(
        f,
        SPAN,
        "66 8B 0D ?? ?? ?? ?? BF FF FF 00 00 66 8B F1 66 03 CF 66 89 0D ?? ?? ?? ??",
        "map_cpu skip counter",
    )?;
    let map_skip = core.ptr_at(sk + 3)?;
    ensure!(core.ptr_at(sk + 21)? == map_skip, "map_cpu skip counter mismatch");
    // The usage function: push ebp ; mov ebp,esp ; push ecx ; call Ticks ; sub eax,[start] ; mov ecx,eax ;
    // sbb edx,[start+4] ; call ToDouble
    let usage = core.find_unique(
        &format!("55 8B EC 51 E8 ?? ?? ?? ?? 2B 05 {} 8B C8 1B 15 {} E8", hx(start), hx(start + 4)),
        "tick usage function",
    )?;
    ensure!(core.rel32(usage + 5)? == ticks, "tick usage function calls another clock");
    // The timer registration: push <thunk to the tick> ... call.
    let site = super::registration(core, f)?;
    let calls = Calls {
        usage: Foreign::at(usage),
        event: Foreign::at(core.rel32(call + 52)?),
        id: core.u32_at(call + 33)?,
        args: [core.ptr_at(call + 47)?, core.ptr_at(call + 41)?, core.ptr_at(call + 28)?, core.ptr_at(call + 22)?],
    };
    let tick = Tick {
        original: Foreign::at(core.ptr_at(site)?),
        ticks: Foreign::at(ticks),
        start,
        tick_count,
        world: core.ptr_at(f + 8)?,
        in_tick,
        version: core.ptr_at(call + 2)?,
        calls,
        cpu,
        cpu_out: core.ptr_at(c3 + 51)?,
        cpu_net: core.ptr_at(c3 + 63)?,
        map,
        map_out,
        map_skip,
    };
    Ok((tick, site))
}
