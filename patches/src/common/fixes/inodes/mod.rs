//! `inodes`: works on filesystems with 64-bit inode numbers (Linux).

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
pub mod linux;

pub static INODES: Feature =
    Feature::fix(Hosts::BOTH, "inodes", "64-bit inode numbers", Tier::Safe, install!(linux: linux::install));
