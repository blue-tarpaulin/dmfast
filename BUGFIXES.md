# Bug fixes

Fixes for bugs in BYOND itself. 

Unlike the [speed patches](PATCHES.md), these change what BYOND does, so they're off by default.

Turn them on when installing:

```
dmfast-install --fixes safe "C:\Program Files (x86)\BYOND"
```

You can also give fix keys, and `-key` leaves one out: `--fixes extended,-tick0`. `--fixes none` turns them off again.

Your choice is saved in `dmfast.ini`, and the `DMFAST_FIXES` environment variable overrides it. Compiler fixes may change the `.dmb` (for example, broken code generation).

| Tier | Meaning |
|---|---|
| `safe` | Fixes the bug and nothing else. |
| `extended` | `safe`, plus fixes a game could notice. |
| `breaking` | Every fix, including ones that turn code stock BYOND accepts into errors, so expect new runtimes. Do not use this on a live server - it's much better for local debugging. |

## Compiler

| Fix | Key | Tier | OS | What changes | How | Bug report |
|---|---|---|---|---|---|---|
| `\` continues one line | `contline` | safe | Both | A `\` at the end of a line joins only the next line, not every blank line after it. | Continuation's whitespace skip stops at a newline | [2990026](https://www.byond.com/forum/post/2990026) |
| No space after a macro | `macrospace` | safe | Both | A macro followed directly by `::`, `:`, `?` or `.` no longer gets a space inserted if it ends in a name, `)`, `]` or a string, so `COLOR::blue` and `MACRO(x)?.var` compile. A single `:` after a name keeps the space, so `c ? MACRO:b` stays a ternary. | Drops trailing macro space before `:`, `?`, `.` (not after a number before `.`, not after a name before a single `:`) | [2976632](https://www.byond.com/forum/post/2976632), [2988673](https://www.byond.com/forum/post/2988673) |
| Operators in type paths | `typepath` | safe | Both | `/obj/!`, `/obj/~`, `/obj/?` and the other operator names are `instruction not allowed here`. | Widens the type-path operator rejection range | [2990024](https://www.byond.com/forum/post/2990024) |
| A type named `world` | `objworld` | safe | Both | A type named `world` below another type (`/obj/world`) keeps its built-in var overrides instead of losing them. | World special case now requires a root parent | [2960599](https://www.byond.com/forum/post/2960599) |
| `MapColors` with 16 arguments | `mapcolors` | safe | Both | `icon.MapColors()` with 16 arguments (a 4×4 matrix without the offset row) works, with 0 for the offsets. Changes BYOND's built-in `/icon` code, so every `.dmb` compiled with it differs from stock. | Swaps in corrected built-in `/icon` source | [2964093](https://www.byond.com/forum/post/2964093) |
| 64-bit inode numbers | `inodes` | safe | Linux | DreamMaker, and DreamDaemon when set there, work on filesystems with 64-bit inode numbers (WSL, some NFS, XFS and btrfs). | Wraps `stat`, `fstat` and `readdir` with their 64-bit calls for the whole process, folding inode numbers into 32 bits | [2991326](https://www.byond.com/forum/post/2991326) |
| Variable id limit | `varlimit` | safe | Both | In a project with about 65,500 variable names, the one whose name gets id 0xFFD0 is no longer mistaken for `.`, so it isn't falsely reported unused and `return x` doesn't compile as `return .`. BYOND's own fix works the same way; output is unchanged for any project with fewer names. | The `$loc` check also tests `$loc`'s register flag | [2991568](https://www.byond.com/forum/post/2991568), [2991894](https://www.byond.com/forum/post/2991894) |
| Proc argument lists | `procargs` | extended | Both | Naming the same argument twice is a `duplicate argument name` error. An argument starting with `/var/` (`/var/x = 5`) is an error instead of silently declaring a global var. | Checks names and leading `/var/` when compiling arguments | [2737322](https://www.byond.com/forum/post/2737322), [2830750](https://www.byond.com/forum/post/2830750) |
| `regex.Find` start position | `regexstart` | breaking | Both | On a global regex, `Find(text, start)` and `Find_char` with an explicit `start` search from there; without one they still continue from the last match. Code that loops on `Find(text, start)` without passing `.next` (e.g. passing the match position or `1`) now finds the same match forever; a Skyrat-derived unit test does this and runs out of memory. `findtext()` itself is unchanged. Changes BYOND's built-in `/regex` code, so every `.dmb` compiled with it differs from stock. | Swaps in corrected built-in `/regex` source | [2885674](https://www.byond.com/forum/post/2885674) |
| Mixed tab/space indentation | `indent` | breaking | Both | A code line whose leading whitespace doesn't match its block's (same, deeper, or an enclosing block's) is an `inconsistent indentation` error, instead of compiling as a different nesting level. Mixing tabs and spaces the same way on every line is fine. | Lexer compares each line's leading whitespace with its block's | [2970135](https://www.byond.com/forum/post/2970135) |

## DreamDaemon

| Fix | Key | Tier | OS | What changes | How | Bug report |
|---|---|---|---|---|---|---|
| Newlines in `url_encode` | `urlcrlf` | safe | Both | `url_encode()` and `list2params()` encode a newline as `%0a`, not `%0d%0a`. | Skips the CRLF branch in URL encoding | [2989601](https://www.byond.com/forum/post/2989601) |
| Large numbers in a database | `dbbignum` | safe | Both | Numbers from 8,321,499,136 to 2^33 (and their negatives) in a database query are stored as numbers instead of NULL. | Corrects the SQL writer's infinity/NaN mask | [2988144](https://www.byond.com/forum/post/2988144) |
| Reusing a closed database | `dbclose` | safe | Both | Before BYOND 516.1670 only: `Close()` or `Open()` on a `/database` that ran queries no longer crashes or corrupts memory, and later `/database` objects on the old file no longer get empty results. | Clears the freed query list; `Open()` closes before taking the new name | [2983292](https://www.byond.com/forum/post/2983292) |
| Failed `call()` leaks arguments | `callref` | safe | Both | A `call()` that fails (`call("missing_proc")(x)`, `call(null)(x)`, also with `arglist()` or named arguments) no longer keeps a reference to each argument, so they can be deleted. | Failure paths release and clear argument slots | [2991514](https://www.byond.com/forum/post/2991514) |
| Profiling procs that error on entry | `profself` | safe | Both | In the profiler, a proc stopped by "Maximum recursion level reached", or by a call with wrong arguments no longer gets a self CPU time and overtime far above its total CPU time (or negative ones). | A new frame's self-time clock starts with its total-time clock | [2913091](https://www.byond.com/forum/post/2913091) |
| Icons modified many times | `iconwrap` | safe | Both | A runtime `/icon` modified thousands of times (once past 64 KB: about 9,400 `Blend`s with a colour) keeps drawing right. `GetPixel()` and others re-ran thousands of operations at every call, freezing the world for seconds per call or outright breaking. | Flattens an icon's operation list over 64 KB once drawn, as BYOND already does elsewhere. | [2922816](https://www.byond.com/forum/post/2922816) |
| `json_encode` and Unicode | `jsonutf` | extended | Both | Non-ASCII text is written as UTF-8 instead of `\uXXXX` escapes, so whatever reads the JSON gets raw UTF-8, including U+2028 and U+2029 (JavaScript before ES2019, such as BYOND 515's browser, rejects them in string literals). | JSON encoder copies UTF-8 bytes unescaped | [2905231](https://www.byond.com/forum/post/2905231) |
| Weighted `pick()` | `pickweight` | extended | Both | `pick()` with constant weights whose last weight is 0 no longer crashes or breaks the world; zero-weight entries are never picked. A constant 0 next to a weight that isn't constant still breaks `pick()`. | Pick opcode skips the last kept option's stray threshold | [2991804](https://www.byond.com/forum/post/2991804) |
| Hub connection loses its socket | `hubsock` | extended | Both | A pending BYOND hub connection keeps its socket while players' connections come and go, so players stop being dropped with "Sequence number N expected but 42 received", and the hub timeout only closes the hub's own socket. | Guards socket callback registration and hub timeout close | None |
| Runtime icon ids | `rscid` | extended | Both | The 65,535th icon a world creates at runtime no longer gets an id another icon already has, so clients don't draw one icon as another. | Skips the reserved id when adding runtime icons | None |
| `world.cpu` and `tick_usage` at start | `tickinit` | extended | Both | `world.tick_usage` is sensible before the first tick, `world.cpu` doesn't carry over a reboot, and the first tick no longer repeats the startup's `world.cpu`. | World reset clears CPU averages, starts tick clock | [2968161](https://www.byond.com/forum/post/2968161), [2968160](https://www.byond.com/forum/post/2968160), [2968159](https://www.byond.com/forum/post/2968159) |
| `/world/Tick` at time 0 | `tick0` | extended | Both | `/world/Tick` also runs at `world.time` 0 (after `world/New`), and again after each reboot. A game whose `Tick` assumes it never runs at 0 sees one extra call. | First-tick hook also calls `/world/Tick` | [2968158](https://www.byond.com/forum/post/2968158) |
| Profiler CPU times and other threads | `profcpu` | extended | Linux | The profiler's self CPU, total CPU and overtime count only the thread that runs procs, not every thread of DreamDaemon (e.g. rust-g, auxmos, etc.) | Profiler clock reads the thread's CPU time instead of the process's | None |
| `num2text` significant figures | `num2sig` | breaking | Both | `num2text(N, Digits)` keeps trailing zeros (`num2text(10, 4)` is `10.00`). `num2text(N)` and `"[N]"` are unchanged. Code that passes a large `Digits` to print a number exactly (timer ids built with `num2text(id, 100)`) gets strings padded to that many digits. | Explicit digits format with `%#g` | [2960272](https://www.byond.com/forum/post/2960272) |
| Comparing with a list or object | `cmperr` | breaking | Both | `<`, `>`, `<=` and `>=` with anything but null, text or a number on the right (a list or object, for example) give a type mismatch runtime error, instead of returning the left side. | Comparisons jump to their type-mismatch error | [2989336](https://www.byond.com/forum/post/2989336), [2983414](https://www.byond.com/forum/post/2983414) |
| `min`/`max` with null | `minmax` | breaking | Both | Null counts as 0 against numbers, and text wins against null, whatever the argument order. Changes results code may rely on. | Wraps min/max pair functions; null counts as 0 | [2816779](https://www.byond.com/forum/post/2816779) |
| Null divided by zero | `nulldiv` | breaking | Both | `null / 0` and `null / null` give "Division by zero", like `0 / 0`. Code that divides `null / null` (an empty reagent holder's purity, for example) now runtimes, and a unit test built on it fails. | `/` handler's null case checks for a zero divisor | [2947411](https://www.byond.com/forum/post/2947411) |
| `initial()` on text or a number | `initvar` | breaking | Both | `initial(x.var)` where `x` is text or a number gives the same runtime error as reading `x.var`, instead of null. | Text/number `initial()` runs the normal var read | [2924945](https://www.byond.com/forum/post/2924945) |
