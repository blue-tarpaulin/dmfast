# Patches

Each patch can be turned off for one run with `DMFAST_DISABLE=<key>` (comma-separated).

Each patch finds the code it changes by signature and turns itself off if anything doesn't match.

These patches don't change what BYOND does. Fixes for BYOND bugs, which do, are opt-in and listed separately in [BUGFIXES.md](BUGFIXES.md).

**Saves** is roughly how much tg gets faster, and how its peak memory changes. Patches overlap & numbers vary by codebase, so treat them as rough.

## Compiler (`dm.exe` / `DreamMaker`)

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
| Parent cache | `parent` | both | ~1% | Caches each node's parent lookup |
| Pair-list intern | `pairs` | both | ~1% | Hash index for pair-list interning |
| Intern rebuild | `rebuild` | both | ~1% | Rebuilds the intern tree less often |
| Char classes | `ctype` | Windows | <1% | Table-based character checks, skipping locale lookups |
| Resource index | `resources` | both | <1% | Indexes resource files by name |
| String index | `strtab` | both | <1%, +21 MB | Hash index for string table lookups |
| List intern | `lists` | both | <1%, +9 MB | Hash index for list interning |
| Vector growth | `vecgrow` | both | <1% | Grows dword arrays geometrically, not one at a time |
| .dmb buffer | `dmbbuf` | both | <1%, +4 MB | Larger write buffer for the .dmb |
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
| .dmb reader | `dmbread` | both | ~17%, ~40% (Linux) | Buffered whole-value .dmb reads, cached ftell |
| Resource-ID index | `resids` | both | ~14% | Hash index for resource ID find-or-add |
| Sorted maps | `sortmap` | both | ~11% | One-pass, exact-size merges; grow maps geometrically |
| String table load | `strtable` | Windows | ~3% | Native string table load; skips the redundant rebuild |
| Var list inheritance | `varlist` | Windows | ~3% | Types without own vars reuse the parent's var list |
| String tree | `strtree` | both | ~2%, +5 MB | Hash index replaces string tree walks |
| Icon state name compare | `dmicmp` | Windows | ~2% | Table-based .dmi icon state name compare |
| CRC-32 | `crc` | both | <1% | Checksums eight bytes per step |

How BYOND itself could fix each of these: [PATCH-FIXES.md](PATCH-FIXES.md).