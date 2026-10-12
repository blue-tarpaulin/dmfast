# Patches

Each patch can be turned off for one run with `DMFAST_DISABLE=<key>` (comma-separated).

**Saves** is roughly how much tg gets faster. Patches overlap & numbers vary by codebase, so treat them as rough.

## Compiler (`dm.exe` / `DreamMaker`)

| Patch | Key | Platforms | Saves | What it does |
|---|---|---|---|---|
| Version banner | `banner` | both | - | Adds a dmfast line to the version banner |
| String table | `strings` | both | ~48% | Hinted free-slot search, geometric string table growth |
| Dir listing | `dirs` | Linux | ~39% (Linux) | Caches directory listings; skips opening non-directories |
| Stdio | `stdio` | both | ~4% | Unlocked file I/O; cached ftell in .dmb writer |
| File cache | `fscache` | Windows | ~2% | Caches file existence, attribute and listing lookups |
| Name filter | `filter` | both | ~2% | Skips scopes that can't contain the name |
| mimalloc | - | both | ~2% | Faster memory allocator |
| Slot hints | `slots` | both | ~2% | Hinted free-slot search in other id tables |
| Parent cache | `parent` | both | ~1% | Caches each node's parent lookup |
| Pair-list intern | `pairs` | both | ~1% | Hash index for pair-list interning |
| Intern rebuild | `rebuild` | both | ~1% | Rebuilds the intern tree less often |
| Char classes | `ctype` | Windows | <1% | Table-based character checks, skipping locale lookups |
| Resource index | `resources` | both | <1% | Indexes resource files by name |
| String index | `strtab` | both | <1% | Hash index for string table lookups |
| List intern | `lists` | both | <1% | Hash index for list interning |
| Vector growth | `vecgrow` | both | <1% | Grows dword arrays geometrically, not one at a time |
| .dmb buffer | `dmbbuf` | both | <1% | Larger write buffer for the .dmb |
| Fast exit | `fastexit` | both | <1% | Exits after saving instead of freeing everything |
| Proto index | `protos` | both | <1% | Hash index for proto lookups |
| Intern index | `internidx` | both | <1% | Hash index for identifier interning |
| Compile-pass leaf skip | `leafskip` | both | <1% | Skips finished leaf nodes in each compile pass |
| Child index | `children` | both | <1% | Indexes children by name for path resolution |
| Progress poll | `poll` | both | up to 0.25 s | Progress thread polls every 10 ms, not 250 |

## DreamDaemon (installed with `--runtime`)

| Patch | Key | Platforms | Saves | What it does |
|---|---|---|---|---|
| mimalloc | - | both | ~29% | Faster memory allocator |
| .dmb reader | `dmbread` | Windows | ~14% | Whole-value .dmb reads from the file buffer; ftell without a syscall |
| Resource-ID index | `resids` | both | ~14% | Hash index for resource ID find-or-add |
| Sorted map insert | `sortmap` | both | ~6% | Fast appends; grows sorted maps geometrically |
| Sorted map merge | `sortmerge` | both | ~4% | Merges sorted maps in one pass |
| String tree | `strtree` | both | ~2% | Hash index replaces string tree walks |
| CRC-32 | `crc` | both | <1% | Checksums eight bytes per step |
| .dmb buffer | `dmbbuf` | Windows | <1% | 1 MB read buffer for the .dmb |
