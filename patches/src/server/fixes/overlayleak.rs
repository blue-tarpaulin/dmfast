//! `overlayleak`: using a `/mutable_appearance` whose overlays or underlays changed (`o.appearance = m`,
//! `o.overlays += m`, reading `m.appearance`) no longer leaks memory on Windows.
//!
//! Turning the mutable appearance's overlay and underlay lists back into an appearance's starts with a `malloc` the
//! size of the list whose result nothing uses (Linux's compiler drops it). The fix skips that call; the size pushed
//! for it is popped as before.

use crate::base::feature::{Feature, Hosts, Tier, install};

pub static OVERLAYLEAK: Feature = Feature::fix(
    Hosts::SERVER,
    "overlayleak",
    "a mutable appearance's overlay lists are converted without a stray malloc",
    Tier::Safe,
    install!(windows: windows::install),
);

#[cfg(windows)]
mod windows {
    use crate::base::feature::Ctx;
    use crate::base::{Result, ensure};

    /// The list conversion: mov edi,[malloc] ; shl eax,2 ; push eax ; call edi ; movzx ecx,word [ebx+0Ch] ;
    /// add esp,4 ; mov ebx,[free_arrays]
    const CONVERT: &str = "8B 3D ?? ?? ?? ?? C1 E0 02 50 FF D7 0F B7 4B 0C 83 C4 04 8B 1D";
    const CALL: usize = 10;

    pub(super) fn install(cx: &Ctx) -> Result<String> {
        let core = cx.core();
        let at = core.find_unique(CONVERT, "overlay list conversion")?;
        ensure!(core.ptr_at(at + 2)? == core.iat_slot("malloc")?, "overlay list conversion: not malloc");
        core.write_code(at + CALL, &[0x90; 2])?;
        Ok(core.report(&[at]))
    }
}
