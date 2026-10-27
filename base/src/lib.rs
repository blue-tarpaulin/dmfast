//! Shared infrastructure for dmfast: features, their installer, and safe access to BYOND's loaded code.

mod error;
pub use error::{Error, Result};

pub mod code;
pub mod deps;
pub mod driver;
pub mod feature;
pub mod image;
pub mod ledger;
#[cfg(target_os = "linux")]
pub mod linux;
pub(crate) mod scan;
pub(crate) mod snapshot;
pub mod sync;
#[cfg(any(test, feature = "validate"))]
pub mod validate;
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::{CORE_NAME, alloc::alloc_status, mem::poke_code, mem::poke_data};
#[cfg(windows)]
pub(crate) use windows::{CORE_NAME, alloc::alloc_status, mem::poke_code, mem::poke_data};
