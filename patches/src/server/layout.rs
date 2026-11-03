//! DreamDaemon's runtime layouts shared by several features, per platform. Each user pins the offsets it reads with
//! its signatures.

/// A movable's appearance id (u32), as SendMaps reads it.
#[cfg(windows)]
pub const THING_LOOK: usize = 0xA4;
#[cfg(target_os = "linux")]
pub const THING_LOOK: usize = 0xA0;

/// The flags word (u16) of a client's SendMaps map record.
pub const RECORD_FLAGS: usize = 0x4C;
/// The record flag "seen this scan": SendMaps' record check sets it, and each tick clears it.
pub const SEEN: u16 = 0x10;
