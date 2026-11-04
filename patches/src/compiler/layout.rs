//! The compiler's object-tree node: where its fields are, per platform. Each user pins the offsets it reads with its
//! signatures. Keyword node ids change between builds (`var` is 0x1D1 / 0x1ED / 0x1FE), so users
//! read them from code and check them, or look them up ([`crate::compiler::names`]).

/// The node's id (u32): its name's string id, or a keyword node's kind (`var`, a proc's argument list).
pub const ID: usize = 0;
/// The kind byte.
pub const KIND: usize = 4;
/// The flags byte (8: indexed, 2: finished).
#[cfg(windows)]
pub const FLAGS: usize = 0x14;
#[cfg(target_os = "linux")]
pub const FLAGS: usize = 0x11;
/// The parent node: the one whose children hold this one (null for the root).
#[cfg(windows)]
pub const PARENT: usize = 0x18;
#[cfg(target_os = "linux")]
pub const PARENT: usize = 0x14;
/// The children: a pointer to an array of node pointers.
#[cfg(windows)]
pub const CHILDREN: usize = 0x1C;
#[cfg(target_os = "linux")]
pub const CHILDREN: usize = 0x18;
/// The child count (u16).
#[cfg(windows)]
pub const COUNT: usize = 0x22;
#[cfg(target_os = "linux")]
pub const COUNT: usize = 0x1E;
