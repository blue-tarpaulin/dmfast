//! DreamSeeker's patches, written into byondcore.dll and byondwin.dll on disk (DreamSeeker loads no other DLL).
//! Every program loads those DLLs, so each rewritten site jumps to a public gate that runs the patch only in
//! dreamseeker.exe and BYOND's original instructions everywhere else, and dmfast.dll puts BYOND's bytes back at
//! every site in the programs it loads into. The audit checks a patched DLL against the stock one: every changed byte
//! is a gate jump or in the added sections, and every patch body keeps to the rules in `validate.rs`.

#[cfg(feature = "build")]
pub mod apply;
#[cfg(feature = "build")]
pub mod audit;
#[cfg(feature = "build")]
pub mod author;
pub mod format;
#[cfg(feature = "build")]
mod gate;
#[cfg(feature = "build")]
pub mod patch;
#[cfg(feature = "build")]
pub mod pe;
#[cfg(feature = "build")]
mod validate;
