//! Byte formats both sides of dmfast read: what the installer (`dmfast-seeker`) writes into BYOND's files and
//! dmfast.dll (`dmfast-base`) reads back at run time. No dependencies, any target.

pub mod fingerprint;
pub mod manifest;
