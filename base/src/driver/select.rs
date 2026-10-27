//! The fix selection: which features are turned on or off. `DMFAST_FIXES` (commas or spaces; even if empty)
//! overrides the `fixes` key of [`CONFIG_FILE`]; `DMFAST_DISABLE` turns features off whatever selects them.

use std::sync::LazyLock;

use crate::driver::Config;
use crate::feature::{Feature, Kind, Status, Tier};

/// Next to dmfast's library (written by `dmfast-install`); its `fixes` key is the selection.
const CONFIG_FILE: &str = "dmfast.ini";

/// The selection entry that lets Experimental features turn on.
const EXPERIMENTAL: &str = "experimental";

/// Entries: feature keys, tier names, `experimental`, or `-key` to leave one out (`none` selects nothing).
struct Selection {
    source: String,
    entries: Vec<String>,
}

static SELECTION: LazyLock<Option<Selection>> = LazyLock::new(|| {
    if let Ok(v) = std::env::var("DMFAST_FIXES") {
        return Some(Selection { source: "DMFAST_FIXES".into(), entries: entries(&v) });
    }
    let path = library_dir()?.join(CONFIG_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    Some(Selection { source: path.display().to_string(), entries: entries(ini_value(&text, "fixes")?) })
});

pub(super) fn selection() -> &'static [String] {
    SELECTION.as_ref().map_or(&[], |s| &s.entries)
}

/// Whether `DMFAST_FIXES` or [`CONFIG_FILE`] gave a selection (even an empty one).
pub(super) fn has_selection() -> bool {
    SELECTION.is_some()
}

/// Whether `entries` turn `f` on, before `DMFAST_DISABLE`, builds and deps: by key unless `-key`; Stable ones also
/// by default or (fixes) a tier at or above theirs; Experimental ones only with `experimental` (fixes: and a tier).
pub(crate) fn wanted(f: &Feature, entries: &[String]) -> bool {
    let has = |e: &str| entries.iter().any(|x| x == e);
    if entries.iter().any(|e| e.strip_prefix('-') == Some(f.key)) {
        return false;
    }
    if has(f.key) {
        return true;
    }
    let tier = f.tier.is_some_and(|t| Tier::ALL.iter().any(|&s| s >= t && has(s.name())));
    match f.status {
        Status::Experimental => has(EXPERIMENTAL) && (f.kind == Kind::Perf || tier),
        _ => f.on_by_default || tier,
    }
}

/// Whether `DMFAST_DISABLE` lists `key` (read like a selection: commas or spaces, any case).
pub(super) fn disabled(key: &str) -> bool {
    std::env::var("DMFAST_DISABLE").is_ok_and(|v| entries(&v).iter().any(|k| k == key))
}

/// Report selection entries that name no feature, tier or `experimental` (keys of every program and OS count).
pub(super) fn check_keys(config: &Config) {
    let Some(s) = SELECTION.as_ref() else { return };
    for e in &s.entries {
        let key = e.strip_prefix('-').unwrap_or(e);
        let known = Tier::ALL.iter().any(|t| t.name() == e) || e == EXPERIMENTAL || config.all().any(|f| f.key == key);
        if !known {
            eprintln!("dmfast: {}: no fix named {key:?}", s.source);
        }
    }
}

/// The value of `key` in INI text (any section; `;` and `#` start comments; the last one wins).
fn ini_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .rev()
        .map(|l| l.split([';', '#']).next().unwrap_or("").trim())
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
}

/// Entries of a selection: separated by commas or spaces, lower-cased; `none` adds nothing.
fn entries(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|e| !e.is_empty() && !e.eq_ignore_ascii_case("none"))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The folder holding this library (dmfast.dll / libdmfast.so).
#[cfg(windows)]
fn library_dir() -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    let mut buf = [0u16; 1024];
    // SAFETY: the address is a function in this module; the buffer length is passed.
    let n = unsafe {
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if GetModuleHandleExW(flags, library_dir as *const u16, &mut module) == 0 {
            return None;
        }
        GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize
    };
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])).parent()?.to_path_buf())
}

#[cfg(target_os = "linux")]
fn library_dir() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: dladdr fills `info` for an address inside a loaded object; dli_fname is checked for null.
    let name = unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(library_dir as *const libc::c_void, &mut info) == 0 || info.dli_fname.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(info.dli_fname)
    };
    Some(std::path::Path::new(std::ffi::OsStr::from_bytes(name.to_bytes())).parent()?.to_path_buf())
}
