# Patches

Each patch can be turned off for one run with `DMFAST_DISABLE=<key>` (comma-separated).

Each patch finds the code it changes by signature and turns itself off if anything doesn't match.

These patches don't change what BYOND does. Fixes for BYOND bugs, which do, are opt-in and listed separately in [BUGFIXES.md](BUGFIXES.md).

**Saves** is roughly how much tg gets faster, and how its peak memory changes. Patches overlap & numbers vary by codebase, so treat them as rough. `map_cpu` numbers are with 50 players.

## Compiler

| Patch | Key | OS | Saves | What it does |
|---|---|---|---|---|
| Version banner | `banner` | both | - | Adds a dmfast line to the version banner |
| String table | `strings` | both | ~48% | Hinted free-slot search, geometric string table growth |
| Dir listing | `dirs` | Linux | ~39% | Caches directory listings; skips needless chdir and opens |
| Stdio | `stdio` | both | ~4% | Unlocked file reads and writes |
| ftell cache | `ftell` | both | <1% | ftell without a seek syscall in the .dmb writer |
| File cache | `fscache` | Windows | ~2%, +13 MB | Caches file existence, attribute and listing lookups |
| Name filter | `filter` | both | ~2%, +3 MB | Skips scopes that can't contain the name |
| mimalloc | `mimalloc` | both | ~2% | mimalloc for small blocks, the system heap for larger ones |
| Slot hints | `slots` | both | ~2% | Hinted free-slot search in other id tables |
| Parent cache | `parent` | both | ~1% | Caches which child sets each type's `parent_type` |
| Pair-list intern | `pairs` | both | ~1% | Hash index for pair-list interning |
| Intern rebuild | `rebuild` | both | ~1% | Rebuilds the intern tree less often |
| Char classes | `ctype` | Windows | <1% | Table-based character checks, skipping locale lookups |
| Resource index | `resources` | both | <1% | Indexes resource files by name |
| String index | `strtab` | both | <1%, +21 MB | Hash index for string table lookups |
| List intern | `lists` | both | <1%, +9 MB | Hash index for list interning |
| Vector growth | `vecgrow` | both | <1% | Grows dword arrays geometrically, not one at a time; tested that nothing reads the spare entries |
| .dmb buffer | `dmbbuf` | both | <1% | Larger write buffer for the .dmb |
| Fast exit | `fastexit` | both | <1% | Exits after saving instead of freeing everything |
| Proto index | `protos` | both | <1%, +13 MB | Hash index for proto lookups |
| Intern index | `internidx` | both | <1% | Hash index for identifier interning |
| Compile-pass leaf skip | `leafskip` | both | <1% | Skips finished leaf nodes in each compile pass |
| Child index | `children` | both | <1% | Indexes children by name for path resolution |
| Progress poll | `poll` | both | up to 0.25 s | Progress thread polls every 10 ms, not 250 |

## DreamDaemon (unless installed with `--no-runtime`)

| Patch | Key | OS | Saves | What it does |
|---|---|---|---|---|
| mimalloc | `mimalloc` | both | ~29% | mimalloc for small blocks, the system heap for larger ones |
| .dmb reader | `dmbread` | both | ~17%, ~40% (Linux) | Buffered whole-value reads (.dmb, also savefiles), cached ftell |
| Proc-table index | `resids` | both | ~14% | Hash index for proc table find-or-add |
| Sorted maps | `sortmap` | both | ~11% | One-pass, exact-size merges; grow maps geometrically |
| String table load | `strtable` | Windows | ~3% | Native string table load; skips the redundant rebuild |
| Var list inheritance | `varlist` | Windows | ~3% | Types without own vars reuse the parent's var list |
| String tree | `strtree` | both | ~2%, +5 MB | Hash index replaces string tree walks |
| Icon state name compare | `dmicmp` | Windows | ~2% | Table-based .dmi icon state name compare |
| CRC-32 | `crc` | both | <1% | Checksums eight bytes per step |
| |
| `icon_states()` | `iconstates` | both | 1.3× | Builds `icon_states()` lists without scanning the list once per state |
| Iconstate copy | `iconcopy` | both | `getFlatIcon` - 2.5× | Copies an icon's frames without making and freeing a zeroed pixel buffer per frame |
| `view()`/`range()` | `locidx` | both | ~N× with N objs/turf | Hash set of the list being built replaces the "already in it?" scan per atom |
| Turf contents prefetch | `turfpf` | both | ~16% `map_cpu` | Fetches the next turfs' contents into cache while map sending checks the current one |
| Map sending | `seenclear` | both | ~12% `map_cpu` | Clears each player's "seen" marks in memory order instead of jumping around |
| Unchanged objects | `mapskip` | both | ~10% `map_cpu` | Skips rebuilding a temporary record for objects a player already sees unchanged |
| Reference counting | `argload` | Linux | ~18% in-round | Faster reference counting: avoids a CPU stall on every call |
| Var reads | `getvar` | Linux | ~7% in-round | Faster `a.b` reads: avoids the same CPU stall as `argload` |
| Turf var chains | `turfvar` | both | ~2.5% in-round (12 z-levels) | Adds new turf var records at the end of their hash chain, so early-loaded z-levels stay near the front |

mimalloc serves the whole process, including `call_ext` DLLs; `HeapWalk` doesn't list its blocks under 4 KB.

How BYOND itself could fix each of these: [PATCH-FIXES.md](PATCH-FIXES.md).

## Native libraries

Tested with stock BYOND and dmfast (default patches and each fix tier) on BYOND 516.1688, Windows and Linux, unless noted.

| Library | Used by | With dmfast | Notes |
|---|---|---|---|
| rust-g 3.x to 7.x | most codebases | Works | tgstation, Bubber, and BeeStation CI unit tests and a 12-minute round match stock. |
| dreamluau | tgstation and forks | Works | With `jsonutf`, its `json_encode` output is raw UTF-8 too. |
| auxtools (current), auxmos 2.x | shiptest, yog | Works | auxtools test suite passes; auxmos atmos under load matches stock. |
| byond-tracy | chomp, vore | Works | Before this release, `tickinit` and `tick0` crashed it on the first tick. On stock too, it crashes on `world.Reboot()` and at shutdown: BYOND unloads it, but its `destroy` does nothing, so its hooks outlive it. |
| Paradise's byond-tracy fork | bay, aurora, paradise, azure | Works | Only on builds it supports (up to 516.1677). |
| Paradise rustlibs (MILLA atmos) | paradise | Works | Tested on Linux 516.1666. Its Windows DLLs don't load on 516.1674 and later, on stock too. |
| vchatlog, verdigris, libmariadb, libmysql, bapi | chomp, vore, aurora, others | Works | vchatlog and verdigris need 516.1674 or later. |
| memorystats, byond_memprofile | monke, vanderlin | Works | Same output as stock. |
| auxlua | bubber (shipped, not loaded) | Broken on stock | A 2024 build that doesn't find BYOND 516. Unused: Lua runs on dreamluau. |
| byond-extools | eris, lc13, sojourn | Broken on stock | Doesn't support BYOND 516. |