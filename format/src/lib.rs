//! Formats both sides of dmfast read: what the installer (`dmfast-seeker`) writes into BYOND's files and
//! dmfast.dll (`dmfast-base`) reads back at run time, and the fix selection both read. No dependencies, any target.

pub mod fingerprint;
pub mod manifest;
pub mod selection;
