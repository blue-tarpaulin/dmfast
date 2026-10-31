//! `profcpu`: on Linux the profiler's CPU times count every DreamDaemon thread, so a proc is charged other threads'
//! CPU (native libraries' jobs, such as rust-g's). The profiler's clock now reads the calling thread's CPU time.
//!
//! Linux only: the Windows profiler's clock isn't moved by other threads.

use crate::base::feature::{Feature, Hosts, Tier, install};

#[cfg(target_os = "linux")]
mod linux;

pub static PROFCPU: Feature = Feature::fix(
    Hosts::SERVER,
    "profcpu",
    "profiler CPU times count only the thread running procs",
    Tier::Extended,
    install!(linux: linux::install),
);
