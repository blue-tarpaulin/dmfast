//! Threads: state that hooks use from one thread only ([`Owned`]), the calling thread's id, and locking.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::Result;

/// Set during install, then handed to the first thread that asks (the compiler thread) and no other; hooks run
/// the original code on every other thread.
pub struct Owned<T> {
    owner: AtomicUsize,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: `value` is written by `set` while no hook runs, then reachable mutably only from the owner.
unsafe impl<T> Sync for Owned<T> {}

impl<T> Default for Owned<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Owned<T> {
    pub const fn new() -> Self {
        Self { owner: AtomicUsize::new(0), value: UnsafeCell::new(None) }
    }

    /// Set the state while a feature installs; `Err` otherwise.
    pub fn set(&self, value: T) -> Result<()> {
        if !crate::ledger::installing() {
            return Err("state set outside an install".into());
        }
        // SAFETY: while a feature installs no other thread runs BYOND's code, so no hook holds the state.
        unsafe { *self.value.get() = Some(value) };
        Ok(())
    }

    /// The state, if set and the calling thread owns it (the first caller becomes the owner). Don't hold the
    /// borrow across a call that can re-enter a hook using the same state.
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    pub fn get(&self) -> Option<&mut T> {
        let me = thread_id();
        let owner = self.owner.load(Relaxed);
        if owner != me && (owner != 0 || self.owner.compare_exchange(0, me, Relaxed, Relaxed).is_err()) {
            return None;
        }
        // SAFETY: only the owner thread gets here; it holds one borrow at a time (caller, see above).
        unsafe { (*self.value.get()).as_mut() }
    }

    /// # Safety
    /// The owner no longer runs hooks (exit-time stats), or only shareable fields (atomics) are touched.
    pub unsafe fn shared(&self) -> Option<&T> {
        unsafe { (*self.value.get()).as_ref() }
    }
}

/// Lock `m`. Builds abort on panic, so a lock is never poisoned.
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Nonzero id of the calling thread without a syscall: the TEB thread id, or `pthread_self()` on Linux.
#[inline(always)]
pub fn thread_id() -> usize {
    #[cfg(windows)]
    {
        let tid: u32;
        // SAFETY: fs:[0x24] is the current thread's TEB ClientId.UniqueThread on i686 Windows.
        unsafe { std::arch::asm!("mov {:e}, fs:[0x24]", out(reg) tid, options(nostack, readonly, preserves_flags)) };
        tid as usize
    }
    #[cfg(not(windows))]
    // SAFETY: pthread_self has no preconditions.
    unsafe {
        libc::pthread_self() as usize
    }
}
