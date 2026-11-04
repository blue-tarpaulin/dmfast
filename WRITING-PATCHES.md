# Writing patches

Everything a patch needs from the shared `base` crate (`base/src`), by task. Each item's own docs say the rest.

## A feature

A `static` `feature::Feature` in `patches/src/<compiler|server|common>/<perf|fixes>/<key>/mod.rs`:
`Feature::perf` or `Feature::fix`, with modifiers like `.stats(..)` and `.after(..)`, and
`install!(windows: windows::install, linux: linux::install)`. List it in `dmfast/src/features.rs` (install order).
An install function gets a `feature::Ctx` and returns its report line (`Image::report(&[sites])`: `at +0x..`). It
finds and checks BYOND's code and records writes; they apply only if the whole install returns `Ok`, all at once
(`ledger`). Optional parts: `Ctx::parts`. A lookup several features start from: `Ctx::shared`.

## Finding BYOND's code: `image::Image` (`cx.core()`, `cx.exe()`, `cx.module(name)`)

- Patterns (IDA style, `??` wildcards): `find_unique`, `find_unique_in`, `find_all` (`Hits::unique`, `first_or`),
  `find_in`, `is_at`.
- Candidates checked before relying on one: `probe_all`, `probe_bytes`, `probe_calls_to`, then `rely(at, len)` on the
  one kept; `probe_unique(ida, what, keep)` and `probe_first` do both.
- Bytes and addresses: `find_bytes`, `find_bytes_in`, `find_data` (outside code), `mentions` (an absolute address in a
  range), `calls_to`, `calls_in`, `reloc_sites`, `code_ranges`. Imports and exports: `iat_slot`, `ucrt_slot`
  (Windows), `import_calls` (Linux), `export`. `code::hx` puts an address into a pattern.

## Reading it

Relied on (a later change to those bytes is a conflict); code as shipped, data as it is now: `bytes`, `array`,
`u8_at`, `u32_at`, `ptr_at` (an absolute operand). Branches: `call_target(at)` (an `E8`), `branch_at(at)` (a `jmp` or
`jcc`: target, next instruction, condition), `jcc_at(at, cond)` (that `jcc` only); raw fields: `rel32`, `rel8`.
Checks: `require(addr, ida, what)` (a pattern), `follow_call(at, ida, what)` (a call and its target's pattern),
`expect` (exact bytes, your error), `is_at` (a `bool`). Not relied on: `haystack` (as shipped), `live_bytes`,
`live_u32`, `live_ptr`, and `c_str` (as earlier features left it).

## Changing it

- Hook a function's entry: `Image::hook(f, len, hook)`, which returns the original to call. The hook has the
  original's signature and calling convention.
- Change an existing call's or jump's target: `Image::retarget(field, target)` (its rel32 field). Make a `jcc` always
  or never jump: `always_jump(at)`, `never_jump(at)`. Write a new 5-byte call or jump over `len` bytes, NOP-padded:
  `Image::call(site, len, target)`, `Image::jump(site, len, target)`.
- Rewrite more than one instruction in place: check `Image::untouched` first (it doesn't see short branches: check the
  code around), then `Image::write_stub(site, len, &stub)` or `Image::write_code` (bytes from `code::branch`,
  `code::pad`); data: `Image::write_data`. Over another feature's write (listed in `after`): `wrap_code`, `wrap_data`.
- New code: a Rust function where the registers allow, a `#[unsafe(naked)]` function with `naked_asm!` (typed
  statics for the addresses it needs: `code::Foreign` for functions, `code::Addr` for places it jumps to and data it
  reads), or a `code::Stub` built one instruction at a time: `bytes`, `bytes_u32` (an embedded dword), and `call`,
  `jmp`, `jcc`, `jmp8`, `jcc8` to an address or a label in it (`label`, `bind`). `code::trampoline` runs instructions
  moved out of a mid-function rewrite (for a function's entry, use `hook`).
- Calling Rust (`extern "C"`) from BYOND's code: arguments pushed right to left and popped after; `eax`, `ecx`,
  `edx`, and the flags are clobbered; on Linux `esp` is 16-byte aligned at the `call`.

## Hooks at run time

- Call BYOND: `code::Foreign`, a typed function pointer set at install.
- What install found: `sync::Installed` (read-only, any thread, set when the install commits). Mutable state the
  first thread to use it owns (others run BYOND's code): `sync::Owned`. Shared mutable state: a `Mutex` with
  `sync::lock`. A value other features read once this one's writes are done: `Ctx::on_commit`.
- Read and write BYOND's memory: `code::read::<T>`, `code::write`, `code::live_u32`. Indexes over its data:
  `hash::Map`, `hash::map()` (const, for statics).

## Errors, checks, and stats

`bail!`, `ensure!`, `Result`. `driver::verify`: `DMFAST_VERIFY=1`, where a feature that caches answers also runs
BYOND's code and counts mismatches (in a sibling `verify.rs`). `driver::stats`: `DMFAST_STATS=1`, where each applied
feature's `Feature::stats` prints its own counters (`driver::Counter`) at exit. Dependency guards on BYOND functions a
feature relies on staying unchanged: `deps`.

## Platform

- `windows`: ucrtbase's exports (`ucrt::ucrt_export`) and `FILE` layout, `module_range`, `Image::crt_alloc`,
  `Image::call_iat_to`.
- `linux`: `linux::symbol`, dmfast's allocator, which the process uses (`linux::alloc`: `malloc`, `realloc`), and
  glibc's `FILE`, stdio functions, and standard streams (`linux::glibc`: `is_standard`).
