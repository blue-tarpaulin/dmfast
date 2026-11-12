<img alt="dmfast" src="docs/banner.png" height="150">

Robusts BYOND's toolchain.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/speedup-faba110b-dark.svg">
  <img alt="dmfast vs BYOND on an SS13 codebase: compile 6.9x faster on Windows and 12x on Linux; DreamDaemon time to world/New 5.5x and 2.4x faster; in-round CPU 15% less on Windows and 28% on Linux; 41% less map_cpu with 16 players on Windows and 49% on Linux" src="docs/speedup-faba110b-light.svg">
</picture>

How much faster depends on your computer and codebase, but expect significant gains.

Supports BYOND 515 and 516. Patches that don't work will disable themselves automatically.

## Install

Download `dmfast-install.exe` from [Releases](https://github.com/blue-tarpaulin/dmfast/releases).

Close BYOND, then double-click `dmfast-install.exe`.

It'll find BYOND and walk you through the setup, or you can drag your BYOND install on top of it. 

From a terminal (the exe also works on Linux):
```shell
dmfast-install.exe "C:\Program Files (x86)\BYOND"
sh dmfast-install.exe ~/byond
```

To turn on the opt-in [bug fixes](BUGFIXES.md), pass `--fixes safe` or `--fixes extended`.

Run it bare via terminal for more commands and help. Re-run it after BYOND updates.

## Uninstall

Double-click the installer again and choose remove, or:

```shell
dmfast-install.exe --uninstall "C:\Program Files (x86)\BYOND"
sh dmfast-install.exe --uninstall ~/byond
```

You can also move each file in `bin/backup/` back over the modified one.

## Check it's working

You can just run the compiler as normal (there's a banner message), or:

```
set DMFAST_STATS=1
dm.exe yourgame.dme
```

To turn patches off, set `DMFAST_DISABLE` to their keys from [PATCHES.md](PATCHES.md), comma-separated.

## Build

See [CONTRIBUTING.md](CONTRIBUTING.md).

If something breaks, please [open an issue](https://github.com/blue-tarpaulin/dmfast/issues).

## License

dmfast is licensed under the BSD 3-Clause License, © 2026 Greytide Worldwide; see [LICENSE](LICENSE).

It bundles [mimalloc](https://github.com/microsoft/mimalloc) (MIT, Microsoft Corporation & Daan Leijen - [vendor/mimalloc-x86/LICENSE.txt](vendor/mimalloc-x86/LICENSE.txt)).

Byte signatures, fingerprints, and any other material identifying or derived from third-party software are not covered by this license 
and remain the property of their respective owners. No rights to such material are granted.
This repository contains no BYOND code or binaries. dmfast is not affiliated with or endorsed by BYOND in any way.

We offer no guarantees or warranty, but every release is checked against stock BYOND for identical output.
