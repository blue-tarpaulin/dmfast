# dmfast

Makes `dm.exe` compile at lightspeed.

Contains multiple patches that will self-disable if not compatible.

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

The path is your BYOND `bin` folder (or `dm.exe` in it) - dragging onto the exe works too.

Run it with no arguments for help. Re-run after BYOND updates.

Add `--runtime` to also speed up DreamDaemon (faster world startup by 4.5×-2×).
Installing again without `--runtime` puts back the stock DreamDaemon.

## Uninstall

You can delete the modified compiler and rename the `_orig` one, or for a full uninstall:

```
dmfast-install.exe --uninstall "C:\Program Files (x86)\BYOND\bin"
```

On Linux: `sh dmfast-install.exe --uninstall ~/byond/bin`.

## Check it's on

```
set DMFAST_STATS=1
dm.exe yourgame.dme
```

Anything that doesn't match your BYOND version prints `disabled` and runs stock.

## Build

The patches live in a private submodule (`patches/`, blue-tarpaulin/dmfast-patches); building needs
access to it:

```
git clone --recurse-submodules https://github.com/blue-tarpaulin/dmfast.git
```

Windows:

```
rustup target add i686-pc-windows-msvc
cargo build --release -p dmfast-install
```

Output: `target/release/dmfast-install.exe` (self-contained).

Linux (needs 32-bit glibc dev files, e.g. `gcc-multilib`, and glibc 2.34 or older):

```
rustup target add i686-unknown-linux-gnu
cargo build --release -p dmfast-install
```

Output: `target/release/dmfast-install` (self-contained).

Single installer for both:

```
py tools/release.py --distro Debian
```

Output: `dist/dmfast-install.exe`. It is the Windows installer with the Linux one appended (gzipped) and a short shell launcher in its DOS header, so Windows runs it as usual and `sh` runs the Linux installer from it (see `packer/src/main.rs`). 

## Release

Push a `v*` tag (e.g. `v4.4.0`). `.github/workflows/release.yml` builds both installers, packs them,
attests the build provenance and publishes the release. Running the workflow by hand builds the same
file as a workflow artifact without publishing it.

