# Patches

Each patch can be turned off with `DMFAST_DISABLE=<key>` (comma-separated).

Fixes for BYOND bugs which change behavior, are opt-in and listed separately in [BUGFIXES.md](BUGFIXES.md).

**Saves** is how much slower tg gets on Windows when only that patch is turned off and all others stay
on, and how much more peak memory the patch uses. `map_cpu` numbers are with ~50 players.

**% of fast** is the saving as a share of our fully-patched time. Some patches overlap, so taking one away can look cheaper than it is.

## Compiler - given stock time of 30 s

| Patch | Key | OS | Saves | % of fast | What it does |
|---|---|---|---|---|---|
| Dir listing | `dirs` | Linux | 14.9 s | 330% | Caches directory listings; skips needless chdir and opens |
| String table | `strings` | both | 14.7 s | 300% | Hinted free-slot search, geometric string table growth |
| Stdio | `stdio` | both | 1.7 s | 35% | Unlocked file reads and writes |
| ftell cache | `ftell` | both | in `stdio`'s | - | ftell without a seek syscall in the .dmb writer |
| mimalloc | `mimalloc` | both | 0.96 s | 20% | mimalloc for small blocks, the system heap for larger ones |
| Parent cache | `parent` | both | 0.74 s | 15% | Caches which child sets each type's `parent_type` |
| Name filter | `filter` | both | 0.74 s, +3 MB | 15% | Skips scopes that can't contain the name |
| Slot hints | `slots` | both | 0.56 s | 11% | Hinted free-slot search in other id tables |
| File cache | `fscache` | Windows | 0.50 s, +13 MB | 10% | Caches file existence, attribute and listing lookups |
| Char classes | `ctype` | Windows | 0.42 s | 9% | Table-based character checks, skipping locale lookups |
| Intern rebuild | `rebuild` | both | 0.31 s | 6% | Rebuilds the intern tree less often |
| List intern | `lists` | both | 0.24 s, +9 MB | 5% | Hash index for list interning |
| List initializer intern | `listinit` | both | 0.21 s | 4% | Hash index for list-valued var initializers |
| String index | `strtab` | both | 0.21 s, +21 MB | 4% | Hash index for string table lookups |
| Resource index | `resources` | both | ~0.1 s | ~2% | Indexes resource files by name |
| Child index | `children` | both | ~0.1 s | ~2% | Indexes children by name for path resolution |
| Fast exit | `fastexit` | both | ~0.1 s | ~2% | Exits after saving instead of freeing everything |
| .dmb buffer | `dmbbuf` | both | ~0.1 s | ~2% | Larger write buffer for the .dmb |
| Intern index | `internidx` | both | ~0.1 s | ~2% | Hash index for identifier interning |
| Compile-pass leaf skip | `leafskip` | both | ~0.1 s | ~2% | Skips finished leaf nodes in each compile pass |
| Vector growth | `vecgrow` | both | ~0.1 s | ~2% | Grows dword arrays geometrically |
| Proto index | `protos` | both | ~0.1 s, +13 MB | ~2% | Hash index for proto lookups |
| Progress poll | `poll` | both | up to 0.25 s | up to 5% | Progress thread polls every 10 ms, not 250 |
| Version banner | `banner` | both | - | - | Adds a dmfast line to the version banner |

## DreamDaemon  - given stock time of 7 s until world/New

| Patch | Key | OS | Saves | % of fast | What it does |
|---|---|---|---|---|---|
| .dmb reader | `dmbread` | both | 0.98 s | 90% | Buffered whole-value reads (.dmb, also savefiles), cached ftell |
| Proc-table index | `procs` | both | 0.92 s | 84% | Hash index for proc table find-or-add |
| Sorted maps | `sortmap` | both | 0.70 s | 64% | One-pass, exact-size merges; grow maps geometrically |
| String tree | `strtree` | both | 0.21 s, +5 MB | 19% | Hash index replaces string tree walks |
| CRC-32 | `crc` | both | 0.05 s | 5% | Checksums eight bytes per step |
| mimalloc | `mimalloc` | both | ~0.04 s | ~4% | mimalloc for small blocks, the system heap for larger ones |
| Icon state name compare | `dmicmp` | Windows | ~0.02 s | ~2% | Table-based .dmi icon state name compare |
| String table load | `strtable` | Windows | ~0.01 s | ~1% | Native string table load; skips the redundant rebuild |
| Var list inheritance | `varlist` | Windows | ~0.01 s | ~1% | Types without own vars reuse the parent's var list |

### In-round

| Patch | Key | OS | Saves | What it does |
|---|---|---|---|---|
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
| byond-tracy | chomp, vore | Works | On stock too, it crashes on `world.Reboot()` and at shutdown: BYOND unloads it, but its `destroy` does nothing, so its hooks outlive it. |
| Paradise's byond-tracy fork | bay, aurora, paradise, azure | Works | Only on builds it supports (up to 516.1677). |
| Paradise rustlibs (MILLA atmos) | paradise | Works | Tested on Linux 516.1666. Its Windows DLLs don't load on 516.1674 and later, on stock too. |
| vchatlog, verdigris, libmariadb, libmysql, bapi | chomp, vore, aurora, others | Works | vchatlog and verdigris need 516.1674 or later. |
| memorystats, byond_memprofile | monke, vanderlin | Works | Same output as stock. |
| auxlua | bubber (unused dlls) | Broken on stock | A 2024 build that doesn't find BYOND 516. Unused: Lua runs on dreamluau. |
| byond-extools | eris, lc13, sojourn | Broken on stock | Doesn't support BYOND 516. |