//! DreamSeeker's patches, written on disk into dreamseeker.exe, byondcore.dll, and byondwin.dll (DreamSeeker loads no
//! other BYOND DLL). Every program loads those DLLs, so each rewritten site jumps to a public gate that runs the patch
//! only in dreamseeker.exe and BYOND's original instructions everywhere else, and dmfast.dll puts BYOND's bytes back
//! at every site in the programs it loads into. The audit checks a patched file against the stock one: every changed
//! byte is a gate jump or in the added sections, and every patch body keeps to the rules in `validate.rs`. The
//! manifest it writes, which dmfast.dll reads too, is `dmfast_format::manifest`.

pub mod apply;
pub mod audit;
pub mod author;
pub mod com;
mod gate;
pub mod patch;
pub mod pe;
mod validate;
