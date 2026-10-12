//! Combines the Windows and Linux installers into one file that is both a Windows executable and a
//! POSIX shell script (the trick Actually Portable Executables use, applied to our own native builds).
//!
//! usage: dmfast-pack <windows installer .exe> <linux installer> <output>
//!
//! Layout of the output:
//! - `MZdmfast='\n` at offset 0. Windows only reads the `MZ` magic and `e_lfanew` (0x3C) of the DOS
//!   header; a shell reads the start as an assignment whose single-quoted value runs to 0x40. The
//!   newline keeps the first line free of NUL bytes, which bash requires before running a file as a
//!   script.
//! - From 0x40, `unpack.sh`: it ends the quote, cuts the gzipped Linux installer off the end of the file,
//!   runs it with the same arguments and exits before the shell reaches any binary data.
//! - The PE headers, moved down from their old place to just after the script, inside the existing
//!   header area (`SizeOfHeaders`), so no section or RVA moves.
//! - The sections, unchanged, then the gzipped Linux installer as trailing data, which Windows ignores.
//!
//! Running it on Linux: `sh dmfast-install.exe ...`, or `./dmfast-install.exe ...` once executable.

use std::io::Write;
use std::process::ExitCode;
use std::{env, fs};

use flate2::Compression;
use flate2::write::GzEncoder;

const MAGIC: &[u8] = b"MZdmfast='\n";
const SCRIPT: &str = include_str!("unpack.sh");
/// Offset of `e_lfanew`, and where the script starts right after it.
const E_LFANEW: usize = 0x3C;
const SCRIPT_AT: usize = 0x40;

fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn pack(win: &[u8], linux_gz: &[u8]) -> Result<Vec<u8>, String> {
    if win.len() < 0x400 || &win[..2] != b"MZ" {
        return Err("Windows installer: not an executable".into());
    }
    let lfanew = u32_at(win, E_LFANEW);
    if win.get(lfanew..lfanew + 4) != Some(b"PE\0\0") {
        return Err("Windows installer: no PE header".into());
    }
    let sections = u16_at(win, lfanew + 6);
    let opt = lfanew + 24;
    let headers_len = 24 + u16_at(win, lfanew + 20) + 40 * sections;
    let size_of_headers = u32_at(win, opt + 60);
    let (dirs, dir_count) = match u16_at(win, opt) {
        0x10B => (opt + 96, u32_at(win, opt + 92)),
        0x20B => (opt + 112, u32_at(win, opt + 108)),
        _ => return Err("Windows installer: unknown optional header".into()),
    };
    for i in 0..dir_count.min(16) {
        let rva = u32_at(win, dirs + 8 * i);
        if i == 4 && rva != 0 {
            return Err("Windows installer is signed; moving its headers would break the signature".into());
        }
        if rva != 0 && rva < size_of_headers {
            return Err(format!("Windows installer: data directory {i} lies in the headers"));
        }
    }
    let table = opt + u16_at(win, lfanew + 20);
    let image_end = (0..sections).map(|i| u32_at(win, table + 40 * i + 20) + u32_at(win, table + 40 * i + 16)).max();
    if image_end != Some(win.len()) {
        return Err("Windows installer already has trailing data".into());
    }

    let script = SCRIPT.replace("\r\n", "\n").replace("@SIZE@", &linux_gz.len().to_string());
    let new_lfanew = (SCRIPT_AT + script.len() + 7) & !7;
    if new_lfanew + headers_len > size_of_headers {
        return Err(format!(
            "launcher script too long: PE headers would end at {:#x}, past SizeOfHeaders {size_of_headers:#x}",
            new_lfanew + headers_len
        ));
    }
    // Inside the quoted value a `'` would end it early.
    if (new_lfanew as u32).to_le_bytes().contains(&b'\'') {
        return Err("PE header offset contains a quote byte".into());
    }

    let mut out = win.to_vec();
    out[..size_of_headers].fill(0);
    out[..MAGIC.len()].copy_from_slice(MAGIC);
    out[MAGIC.len()..E_LFANEW].fill(b' ');
    out[E_LFANEW..SCRIPT_AT].copy_from_slice(&(new_lfanew as u32).to_le_bytes());
    out[SCRIPT_AT..SCRIPT_AT + script.len()].copy_from_slice(script.as_bytes());
    out[new_lfanew..new_lfanew + headers_len].copy_from_slice(&win[lfanew..lfanew + headers_len]);
    out.extend_from_slice(linux_gz);
    Ok(out)
}

fn run(args: &[String]) -> Result<String, String> {
    let [win, linux, out] = args else {
        return Err("usage: dmfast-pack <windows installer .exe> <linux installer> <output>".into());
    };
    let read = |p: &String| fs::read(p).map_err(|e| format!("{p}: {e}"));
    let (win_bytes, linux_bytes) = (read(win)?, read(linux)?);
    if !linux_bytes.starts_with(b"\x7fELF") {
        return Err(format!("{linux}: not an ELF executable"));
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&linux_bytes).and_then(|()| gz.flush()).map_err(|e| e.to_string())?;
    let linux_gz = gz.finish().map_err(|e| e.to_string())?;
    let packed = pack(&win_bytes, &linux_gz)?;
    fs::write(out, &packed).map_err(|e| format!("{out}: {e}"))?;
    Ok(format!(
        "{out}: {} bytes (Windows {} + Linux {} gzipped to {})",
        packed.len(),
        win_bytes.len(),
        linux_bytes.len(),
        linux_gz.len()
    ))
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dmfast-pack: {e}");
            ExitCode::FAILURE
        }
    }
}
