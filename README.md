<img alt="dmfast" src="docs/banner.png" height="150">

Robusts BYOND's compiler.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/speedup-921cf31c-dark.svg">
  <img alt="dmfast speedups on SS13 codebases: compile 6.6x faster on Windows and 10.2x on Linux; DreamDaemon world load 4.4x and 2.1x faster" src="docs/speedup-921cf31c-light.svg">
</picture>

How much faster depends on the computer and codebase, but expect significant gains.

Byond 515 and 516 are supported. Patches that don't work will just disable themselves.

## Install

Download `dmfast-install.exe` from [Releases](https://github.com/blue-tarpaulin/dmfast/releases).

Close BYOND, then:

```
dmfast-install.exe "C:\Program Files (x86)\BYOND\bin"
```

The same file installs on Linux:

```
sh dmfast-install.exe ~/byond/bin
```

The path is your BYOND `bin` folder (or `dm.exe`) - dragging the exe onto the installer works too.

Run it with no arguments for help. Re-run after BYOND updates.

Add `--runtime` to also speed up DreamDaemon (while it's in beta):

```
dmfast-install.exe --runtime "C:\Program Files (x86)\BYOND\bin"
```

Installing again without `--runtime` puts back the stock DreamDaemon.

## Uninstall

You can delete each modified program and rename the `_orig` copy back, or for a full uninstall:

```
dmfast-install.exe --uninstall "C:\Program Files (x86)\BYOND\bin"
```

On Linux: `sh dmfast-install.exe --uninstall ~/byond/bin`.

## Check it's working

You can just run the compiler as normal, or:

```
set DMFAST_STATS=1
dm.exe yourgame.dme
```

## Build

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

dmfast is licensed under the BSD 3-Clause License, © 2026 Greytide Worldwide; see [LICENSE](LICENSE).

It bundles [mimalloc](https://github.com/microsoft/mimalloc) (MIT, Microsoft Corporation & Daan Leijen - [vendor/mimalloc-x86/LICENSE.txt](vendor/mimalloc-x86/LICENSE.txt)).

This repository contains no BYOND code or binaries. 

dmfast changes your own installed copy of BYOND in memory while it runs. 
The installer only makes the BYOND programs load dmfast and keeps their originals, so uninstalling restores stock BYOND.

dmfast is not affiliated with or endorsed by BYOND in any way.
