# Bug fixes

Fixes for bugs in BYOND itself. Unlike the speed patches in [PATCHES.md](PATCHES.md), these change what BYOND does, so they are off unless you turn them on: set `DMFAST_FIXES` to their keys, comma-separated, or to `all`. `DMFAST_DISABLE=<key>` leaves one out of `all`.

- **Compiler fixes** apply when `DMFAST_FIXES` is set while compiling, and can change the `.dmb`. The compiler's dmfast line says how many applied, e.g. `(21/21, 2 fixes)`.
- **DreamDaemon fixes** need dmfast installed with `--runtime`, and `DMFAST_FIXES` set when DreamDaemon starts.

Each fix finds the code it changes by signature. If anything doesn't match, it turns itself off and prints why. With `DMFAST_STATS=1`, the `fixed=` part of the stats line lists the fixes that applied, or says `fixed=unselected` if `DMFAST_FIXES` isn't set.

## Compiler (`dm.exe` / `DreamMaker`)

| Fix | Key | OS | Bug report | What changes |
|---|---|---|---|---|

None yet.

## DreamDaemon

| Fix | Key | OS | Bug report | What changes |
|---|---|---|---|---|

None yet.
