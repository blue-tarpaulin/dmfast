# Changelog

Write changes under `## Unreleased`. Before tagging a release, rename that heading to the version; the release notes include that section's text as is.

## Unreleased

- **New patch:** `truth` (DreamDaemon, Linux): faster `if(x)`, `!x`, `&&`, and `||` on numbers. ~1% less CPU usage in-round.
- **New patch:** `varread` (DreamDaemon, Windows): faster reads of user-defined vars by name. ~1% less CPU usage in-round.
- **Changed:** the list initialization patch key `pairs` is now `listinit`.
- **Changed:** `jsonutf` is now in the `extended` tier since its output can differ from stock.

## 7.0.0

- **Fixed:** dmfast's allocator (Windows) no longer tears itself down at process exit to prevent some edge cases.
- **Fixed:** `initvar` (Linux) now installs alongside the `getvar` patch.
- **Changed:** the fix selection accepts `experimental`, which turns on experimental fixes of the chosen tiers.
- **Changed:** the `DMFAST_STATS` summary line lists keys instead of bit masks.
- **Changed:** the proc-table index patch key `resids` is now `procs`.
- **Changed:** internal reordering and restructuring of the patch system.

## 6.3.0

- **New patch:** `argload` (DreamDaemon, Linux): faster reference counting. ~18% less CPU usage in-round.
- **New patch:** `seenclear` (DreamDaemon): faster map sending by clearing "seen" marks in memory order. ~12% less `map_cpu` with 50 players.
- **New patch:** `mapskip` (DreamDaemon): faster map sending by skipping objects a player already sees unchanged. ~10% less `map_cpu` with 50 players.
- **New patch:** `getvar` (DreamDaemon, Linux): faster var reads by avoiding the same CPU stall as `argload`. ~7% less CPU usage in-round.
- **New patch:** `turfvar` (DreamDaemon): faster turf var reads on multi-z maps by keeping each turf hash chain in z-level order. ~2.5% less CPU in-round on a 12-z map.
- **New patch:** `turfpf` (DreamDaemon): faster map sending by loading the next turfs' contents into cache ahead of time. ~16% less `map_cpu` with 50 players.
- **New patch:** `locidx` (DreamDaemon): faster `view()`/`range()` by indexing the list being built. ~N× faster with N objects/turf.
- **Internal:** dmfast's allocator (Linux) returns at once on `free(NULL)`, which BYOND does three times per proc return. ~3% faster small proc calls.
- **Internal:** dmfast's allocator (Linux) runs mimalloc's full-heap collect walk 100× less often. ~7% less CPU in heavy allocation churn.

## 6.2.0

- **New patch:** `iconcopy` (DreamDaemon, 516.1663+): drawing a state from a .dmi no longer makes and frees a zeroed pixel buffer for every frame of the file. `getFlatIcon` on a dressed human is 2.7× faster.
- **New patch:** `iconstates` (DreamDaemon, 516): `icon_states()` no longer scans its list once per state, which made it quadratic in the number of states. 1.3× faster.
- **New fix:** `profself` (safe): in the profiler, a proc stopped by "Maximum recursion level reached" or by a call with wrong arguments no longer gets a self CPU time far above its total CPU time, or a negative one.
- **New fix:** `profcpu` (extended, Linux): the profiler's CPU times count only the thread running procs.
- **New fix:** `iconwrap` (safe): a runtime `/icon` modified tens of thousands of times no longer freezes the server or breaks.

## 6.1.0

- **New fix:** `dbclose` (safe) backports BYOND 516.1670's `/database` fix to older builds, where reusing a closed database crashed stock BYOND and silently corrupted memory under dmfast's allocator.
- **Fixed:** `dm -o` and `dm -code_tree` could crash: they compile twice in one process, and the proto index still pointed into memory the first compile had freed.
- **Fixed:** the `pickweight` fix missed many picks whose last weight is 0, e.g. `pick(1;"a", 1;"b", 1;"c", 1;"d", 0;"e")` still crashed the world.
- **Fixed:** the `callref` fix now also covers the other failed `call()`s: a null, number or datum callee, `arglist()` or named arguments, and `call_ext()` libraries.
- **Fixed:** the `macrospace` fix broke `c ? MACRO:b`; a single `:` after a macro keeps its space again.
- **Fixed:** with `tick0`, the extra `/world/Tick` at time 0 saw a huge `world.tick_usage`.
- **Fixed:** on Linux, with `inodes`, a `readdir` entry could be overwritten by later reads of other directories on the same thread.
- **Fixed:** with the `tickinit` or `tick0` fix on, starting byond-tracy crashed DreamDaemon on the first tick.
- The DM test suite runs about 10× faster and finishes on Windows. A stock crash that dmfast hides fails the run unless it is a known BYOND bug fixed in later builds (then a warning).

## 6.0.1

- **Fixed:** on Linux, the `callref` fix broke `call("name")(...)`: every call by name failed with "bad proc".
- `num2sig` moves to the `breaking` tier: `num2text(N, 100)`, used to print ids exactly, became a 100-digit string.
- CI runs regression worlds (`tests/fixes/keep`) and the DM test suite (spacestation13/dm-test-suite) against stock BYOND and dmfast.

## 6.0.0

- **Opt-in bug fixes:** dmfast now carries 24 fixes for BYOND bugs, off unless you turn them on. 
  - For example, `dmfast-install --fixes safe`. `safe` or `extended` is recommended. See [BUGFIXES.md](BUGFIXES.md).
- **Guided install:** double-click the installer (or run it bare in a Linux term) to set it up step by step.

## 5.0.0

- **DreamDaemon is sped up by default.** The installer now patches DreamDaemon too; pass `--no-runtime` to leave it stock. `--runtime` is no longer needed (still accepted).
- **Less memory:** dmfast's allocator now uses mimalloc only for small blocks and the system heap for larger ones: about 100 MB less on a large world load, same speed. `mimalloc-redirect32.dll` is no longer installed; upgrading removes it.
- **Sorted maps:** the `sortmerge` key is gone; `DMFAST_DISABLE=sortmap` now turns off both the merge and the insert patch.
- **Removed:** DreamDaemon's `.dmb` read buffer, which measured no gain. The compiler's `dmbbuf` stays.

## NEXT MAJOR VERSION

- Rename the `resids` patch key to `procids`
