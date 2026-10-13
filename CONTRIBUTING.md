# Contributing

## Build

The patches live in a private submodule (`patches`); building needs access to it. 

To ask for access, [open an access request](https://github.com/blue-tarpaulin/dmfast/issues/new?template=access_request.yml). We'll give access to community members we trust.

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

Output: `dist/dmfast-install.exe` - works on both operating systems.

## Bug fixes

Fixes for BYOND bugs ([BUGFIXES.md](BUGFIXES.md)) are kept apart from the speed patches, which must never change output:

- **Code:** `patches/src/<windows|linux>/fixes/<key>.rs`, with logic both platforms share in `patches/src/fixes/`. Register it in that platform's `fixes/mod.rs` (`install_compiler` or `install_runtime`, with `groups::apply_fix`), and add its key to `KEYS` there. A fix on both platforms keeps the same key and bit on each.
- **Opt-in:** a fix only installs when `DMFAST_FIXES` lists its key or `all`. Its bit goes in `dmfast_fixed`, not `dmfast_patched`. `DMFAST_DISABLE` covers both patches and fixes, so a fix's key must not be one of the keys in [PATCHES.md](PATCHES.md).
- **Test:** `tests/fixes/<key>.dme` is a world that shows the bug, logs its results with `world.log`, and calls `shutdown()`. `tests/fixes/<key>.expected` is what it must log with the fix on. `tools/fixtest.sh <bin dir with dmfast installed with --runtime> [key]` checks that stock BYOND doesn't give that output and the fix does, alone and with `DMFAST_FIXES=all`. CI runs every fix's test on each BYOND build in the release matrix.
- **Docs:** add a row to [BUGFIXES.md](BUGFIXES.md) and a line to [CHANGELOG.md](CHANGELOG.md).

## Release

Rename `## Unreleased` in [CHANGELOG.md](CHANGELOG.md) to the version (e.g. `## 4.4.0`), commit, then push a `v*` tag (e.g. `v4.4.0`). The release notes include that section.
