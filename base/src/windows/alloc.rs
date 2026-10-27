//! dmfast's allocator: mimalloc.dll (vendor/mimalloc-x86/hybrid.c) hooks the C runtime's heap when it loads,
//! sending small blocks to mimalloc and large ones to the process heap. Each patched program imports dmfast.dll
//! first, and dmfast.dll imports mimalloc.dll, so it loads before the program and byondcore allocate. It reads
//! `DMFAST_DISABLE=mimalloc` itself and then leaves the stock heap alone.

#[link(name = "mimalloc", kind = "raw-dylib")]
unsafe extern "C" {
    fn mi_version() -> i32;
    /// Whether the allocator hooks are in.
    fn mi_is_redirected() -> i32;
}

/// mimalloc's version; the export the installer adds to each patched program's imports, so this DLL loads first.
#[unsafe(no_mangle)]
pub extern "C" fn dmfast_mimalloc_version() -> i32 {
    // SAFETY: mimalloc.dll's export, no arguments.
    unsafe { mi_version() }
}

/// How the allocator runs, for the stats line.
pub(crate) fn alloc_status() -> String {
    // SAFETY: mimalloc.dll's export, no arguments.
    if unsafe { mi_is_redirected() } != 0 {
        crate::driver::mimalloc(dmfast_mimalloc_version(), "heap from 4 KB")
    } else {
        "mimalloc off".into()
    }
}
