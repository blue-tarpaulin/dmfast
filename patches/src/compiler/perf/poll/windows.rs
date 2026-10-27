use crate::base::Result;
use crate::base::feature::Ctx;
use crate::base::image::Image;

/// Whether this dm.exe has the progress thread (builds before 516.1656 don't).
pub fn has_progress_thread(exe: &Image) -> bool {
    exe.iat_slot("?Sleep@DungThread@@SAXK@Z").is_ok()
}

pub fn install(cx: &Ctx) -> Result<String> {
    let exe = cx.exe()?;
    // push 250 ; call [Sleep] ; mov ecx,[ebp-x] ; lea eax,[ebp-y]
    let m = exe.find_unique("68 FA 00 00 00 FF 15 ?? ?? ?? ?? 8B 4D ?? 8D 85", "progress Sleep(250)")?;
    exe.write_code(m + 1, &10u32.to_le_bytes())?;
    Ok("10 ms".into())
}
