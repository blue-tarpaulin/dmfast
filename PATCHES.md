# Patches

Each patch can be turned off with `DMFAST_DISABLE=<key>` (comma-separated).

Fixes for BYOND bugs which change behavior, are opt-in and listed separately in [BUGFIXES.md](BUGFIXES.md).

**Saves** is how much slower tg gets on Windows when only that patch is turned off and all others stay
on, and how much more peak memory the patch uses. `map_cpu` numbers are with ~50 players.

**% of fast** is the saving as a share of our fully-patched time. Some patches overlap, so taking one away can look cheaper than it is.

## Compiler - given stock time of 30 s

| Patch | Key | OS | Saves | % of fast | What it does |
|---|---|---|---|---|---|
| Dir listing | `dirs` | Linux | 12.2 s | 490% | Caches directory listings; skips needless chdir and opens |
| String table | `strings` | both | 18.2 s | 430% | Hinted free-slot search, geometric string table growth |
| Stdio | `stdio` | both | 1.49 s | 35% | Unlocked file reads and writes |
| ftell cache | `ftell` | both | in `stdio`'s | - | ftell without a seek syscall in the .dmb writer |
| mimalloc | `mimalloc` | both | 0.69 s | 16% | mimalloc for small blocks, the system heap for larger ones |
| Slot hints | `slots` | both | 0.68 s | 16% | Hinted free-slot search in other id tables |
| Name filter | `filter` | both | 0.61 s, +3 MB | 15% | Skips scopes that can't contain the name |
| File cache | `fscache` | Windows | 0.61 s, +18 MB | 14% | Caches file existence, attribute and listing lookups |
| Intern rebuild | `rebuild` | both | 0.54 s | 13% | Rebuilds the intern tree less often |
| Parent cache | `parent` | both | 0.52 s | 12% | Caches which child sets each type's `parent_type` |
| Char classes | `ctype` | Windows | 0.32 s | 8% | Table-based character checks, skipping locale lookups |
| List intern | `lists` | both | 0.33 s, +10 MB | 8% | Hash index for list interning |
| List initializer intern | `listinit` | both | 0.30 s | 8% | Hash index for list-valued var initializers |
| Resource index | `resources` | both | 0.19 s | 5% | Indexes resource files by name |
| Fast exit | `fastexit` | both | 0.18 s | 5% | Exits after saving instead of freeing everything |
| Progress poll | `poll` | both | up to 0.25 s | up to 4% | Progress thread polls every 10 ms, not 250 |
| String index | `strtab` | both | ~0.1 s, +26 MB | ~3% | Hash index for string table lookups |
| Intern index | `internidx` | both | ~0.1 s | ~3% | Hash index for identifier interning |
| Child index | `children` | both | ~0.1 s | ~2% | Indexes children by name for path resolution |
| Proto index | `protos` | both | ~0.1 s, +13 MB | ~2% | Hash index for proto lookups |
| .dmb buffer | `dmbbuf` | both | ~0.1 s | ~1% | Larger write buffer for the .dmb |
| Compile-pass leaf skip | `leafskip` | both | ~0.1 s | ~1% | Skips finished leaf nodes in each compile pass |
| Vector growth | `vecgrow` | both | ~0.1 s | ~1% | Grows dword arrays geometrically |
| Version banner | `banner` | both | - | - | Adds a dmfast line to the version banner |

## DreamDaemon  - given stock time of 7 s until world/New

| Patch | Key | OS | Saves | % of fast | What it does |
|---|---|---|---|---|---|
| Sorted maps | `sortmap` | both | 2.42 s, +6 MB | 210% | One-pass, exact-size merges; grow maps geometrically |
| .dmb reader | `dmbread` | both | 1.16 s | 100% | Buffered whole-value reads (.dmb, also savefiles), cached ftell |
| Proc-table index | `procs` | both | 1.04 s | 91% | Hash index for proc table find-or-add |
| String tree | `strtree` | both | 0.23 s, +5 MB | 20% | Hash index replaces string tree walks |
| CRC-32 | `crc` | both | 0.07 s | 6% | Checksums eight bytes per step |
| Var list inheritance | `varlist` | Windows | 0.04 s | 4% | Types without own vars reuse the parent's var list |
| mimalloc | `mimalloc` | both | 0.04 s, +8 MB | 3% | mimalloc for small blocks, the system heap for larger ones |
| Icon state name compare | `dmicmp` | Windows | 0.03 s | 3% | Table-based .dmi icon state name compare |
| String table load | `strtable` | Windows | ~0.02 s | ~2% | Native string table load; skips the redundant rebuild |

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
| Var lookups | `varread` | Windows | ~1.2% in-round | Caches each type's var defaults, so `a.b` skips BYOND's searches through the type |
| Truth tests | `truth` | Linux | ~1.1% in-round | Tests numbers in `if`, `!`, `&&`, and `\|\|` with integer instructions instead of x87 |

How BYOND itself could fix each of these: [PATCH-FIXES.md](PATCH-FIXES.md).

## Native libraries

Tested with stock BYOND and dmfast (default patches and each fix tier) on BYOND 516.1688, Windows and Linux, unless noted.

mimalloc serves the whole process, including `call_ext` DLLs; `HeapWalk` doesn't list its blocks under 4 KB.

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