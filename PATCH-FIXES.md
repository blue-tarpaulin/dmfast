# How BYOND could fix these

Each dmfast patch works around a slow spot in BYOND. Fixing them in BYOND would be simpler and faster than patching from outside.

Most of the problems are one of three kinds:

- **Searching from the start every time.** Finding a free slot or a matching item by checking every entry from the beginning. This gets slower as the table grows, so over a whole compile it's slower still.
- **Growing by one.** Making an array one entry (or ten) bigger each time it fills up, so it gets copied over and over. Growing it by a fraction of its size instead (dmfast uses 2× for most tables, 1.125× for sorted maps) fixes this ([geometric expansion](https://en.wikipedia.org/wiki/Dynamic_array#Geometric_expansion_and_amortized_cost)).
- **Lookup trees that never get balanced.** Binary search trees fed mostly sorted input turn into long chains. A hash table or a balanced binary tree fixes this.

## Estimated wins

**Saves** is how much slower things get when only that fix is taken away and all the others stay in, on tgstation (Windows). MB is how much more peak memory the fix uses.

**% of fast** is the saving as a share of our fully-patched time: about 4.4 s for the compile, 1.05 s for the world load. Some patches overlap (one covers part of what another does), so taking one away can look cheaper than it is.

## Compiler - given stock time of ~27 s

| Area | Saves | % of fast | What's slow | Fix |
|---|---|---|---|---|
| Directory listings (Linux) | 13.4 s | 330% | Matching file names case-insensitively lists every directory on the way, then opens each entry to see whether it's a directory. | Cache each listing until the directory changes, and take the file type from the listing. |
| String table | 13.2 s | 300% | Adding a string scans for a free slot from the start, and the table grows 10 slots at a time. | Start from the lowest slot that might be free. Double the table when full. |
| File reading and writing | 1.5 s | 35% | Files are read and written a character at a time through locking calls, and the `.dmb` writer asks the OS for the file position before every string. | Use the non-locking calls (one thread uses these files) or read and write in blocks. Track the position yourself. |
| Memory allocator | 0.86 s | 20% | The system allocator takes a lock on every call. | Use a faster allocator, such as [mimalloc](https://github.com/microsoft/mimalloc), with a heap per thread, for small blocks; keep large ones on the system heap, which doesn't round them up. |
| Parent lookup | 0.67 s | 15% | Finding a type's parent scans its children (up to ~1,400) every time. | Store the parent on the type, or cache it until the children change. |
| Name resolution | 0.67 s, +3 MB | 15% | Resolving a name checks every child of every enclosing scope, including scopes that don't declare it. | Keep a set of the names each scope declares, and skip scopes without the name. |
| Other id tables | 0.50 s | 11% | They scan for a free slot from the start, like the string table. | Start from the lowest slot that might be free. Growing by 10 is fine here: these tables stay small. |
| File lookups (Windows) | 0.45 s, +13 MB | 10% | The same file existence and attribute questions go to the OS over and over. | Cache the answers for the compile, and clear the cache when the compiler writes a file. |
| Character checks (Windows) | 0.38 s | 9% | Checks like "is this a letter" look up the locale on every call. | Use a 128-entry [lookup table](https://en.wikipedia.org/wiki/Lookup_table) for ASCII. |
| Identifier tree rebuilds | 0.28 s | 6% | The identifier tree is rebuilt from scratch every 256 inserts. | Rebuild less often, or replace the tree with a hash table. |
| Id lists | 0.22 s, +9 MB | 5% | Id lists are found through a tree that only rebalances at depth 256. | Use a hash table keyed by the list's contents. |
| Pair lists | 0.19 s | 4% | Finding a pair list checks every stored list, and storage grows one entry at a time. | Use a hash table. Double the storage when full. |
| String lookup | 0.19 s, +21 MB | 4% | Finding an existing string walks an unbalanced tree, comparing strings at each step. | Use a hash table from string to id. |
| Resource lookup | ~0.1 s | ~2% | Finding a resource by file name compares it with each resource in turn. | Use a hash table from file name to resource. |
| Child lookup | ~0.1 s | ~2% | Finding a child by name scans all of the type's children. | Give types with many children a name-to-child index. |
| Exit | ~0.1 s | ~2% | After saving, the compiler frees every object one at a time before exiting. | Exit once the `.dmb` is saved; the OS frees everything at once. |
| `.dmb` write buffer | ~0.1 s | ~2% | The `.dmb` is written in 4 KB pieces. | Use a 1 MB buffer. |
| Identifier lookup | ~0.1 s | ~2% | Identifier and path lookups walk a tree, comparing strings at each step, though most find an existing entry. | Use a hash table. |
| Compile passes | ~0.1 s | ~2% | Every pass visits every node, including finished leaves that have nothing left to do. | Skip finished nodes that have no children. |
| Id arrays | ~0.1 s | ~2% | Appending to an id array grows it by exactly one entry. | Double the capacity when full. |
| Proto lookup | ~0.1 s, +13 MB | ~2% | Lookups walk a tree comparing whole key arrays, though most find an existing entry. | Put a hash table in front of the tree. |
| Progress check | up to 0.25 s | up to 6% | The progress thread polls every 250 ms, so a finished compile can idle up to 0.25 s. | Poll every 10 ms, or signal the thread when the compile finishes. |

## DreamDaemon - given stock time of ~6.7 s

| Area | Saves | % of fast | What's slow | Fix |
|---|---|---|---|---|
| `.dmb` reading | 0.94 s | 90% | The `.dmb` is read a byte at a time through locking calls, and the reader asks the OS for the file position before every string. | Read whole values, or the whole file, at once. Track the position yourself. |
| Proc table | 0.88 s | 84% | While the world loads, each proc it registers is compared with every proc already in the table. | Use a hash table. |
| Sorted maps | 0.67 s | 64% | Each insert into a small sorted map reallocates it to exactly one more entry, and merging two maps inserts entries one at a time, shifting and reallocating each time. | Grow the array geometrically when full. Merge both sorted arrays in one pass ([merge algorithm](https://en.wikipedia.org/wiki/Merge_algorithm)) into an array of the exact size. |
| String lookup | 0.20 s, +5 MB | 19% | Every string lookup walks an unbalanced tree. | Use a hash table. |
| CRC-32 checksum | 0.05 s | 5% | The checksum is computed one byte per step. | Process eight bytes per step ([slicing-by-8](https://en.wikipedia.org/wiki/Computation_of_cyclic_redundancy_checks#Byte-slicing_using_multiple_tables)); same result. |
| Memory allocator | ~0.04 s | ~4% | Same as the compiler. | Same as the compiler. |
| String table load (Windows) | ~0.01 s | ~1% | After the load, a rebuild re-checks every entry's checksum (always passing) and relinks the tree the load just built. | Skip the re-check after a fresh load. With a hash table for string lookups, skip the relink too. |
| Var list inheritance (Windows) | ~0.01 s | ~1% | A type with no vars of its own rebuilds its parent's var list and gets the same list back. | Reuse the parent's list. |
| Icon state names (Windows) | ~0.02 s | ~2% | Comparing icon state names makes locale lookups for every character. | Use 256-entry lookup tables. |
| |
| `icon_states()` lists | 1.3x | - | Each state name goes in with `L[name] = null`, and every key the list doesn't have yet is first searched for in the whole list: quadratic in the number of states. | The builder's list only gets keys through that set, so a key the tree misses is new. |
| Icon state copy | `getFlatIcon` - 2.5x | - | Drawing one state from a .dmi copies the whole file's icon, and copying each frame first makes a zeroed buffer of the world's icon size, then frees it to share the source frame's pixels. | Copy only the state being drawn. |
| Reference counting (Linux) | ~18% in-round | - | The functions that add and drop a value's reference read their 8-byte argument in one load, but callers wrote it as two 4-byte halves, so the CPU stalls on every call. | Read the argument as two halves (as the Windows build does), or pass values by pointer. |
| Map sending | ~12% `map_cpu` (50 players) | - | Every tick, for every player, map sending clears a "seen" mark on each object that player knows about by following a list in id order, which jumps all over memory. | Clear the marks in memory order, or use a tick counter instead of a mark that needs clearing. |

## Memory

Most of these speedups trade some memory for speed. Measured on Windows with large SS13 codebases:

| | Stock peak (private) | dmfast peak (private) |
|---|---|---|
| Compile | 433 MB | 483 MB |
| Daemon - world load<br>(small codebase) | 358 MB | 370 MB |
| Daemon - world load<br>(large codebase) | 690 MB | 710 MB |

Most of the compiler's extra is the lookup tables the speedups add. You can always disable individual patches, though keep in mind some overlap.