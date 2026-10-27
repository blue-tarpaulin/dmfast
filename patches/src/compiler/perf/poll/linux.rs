use crate::base::Result;
use crate::base::feature::Ctx;
use crate::base::image::Image;

/// Whether this DreamMaker has the progress thread (builds before 516.1656 don't)
pub fn has_progress_thread(host: &Image) -> bool {
    host.probe_bytes(b"_ZN10DungThread5SleepEm\0").is_ok_and(|h| !h.is_empty())
}

/// The progress thread polls with `DungThread::Sleep(250)`.
pub fn install(cx: &Ctx) -> Result<String> {
    let host = cx.exe()?;
    // push 250 ; call DungThread::Sleep ; lea eax,[ebp-x] ; pop ecx ; pop edi ; push eax ; push [ebp+8]
    let m = host.find_unique("68 FA 00 00 00 E8 ?? ?? ?? ?? 8D 45 ?? 59 5F 50 FF 75 08", "progress Sleep(250)")?;
    host.write_code(m + 1, &10u32.to_le_bytes())?;
    Ok("10 ms".into())
}
