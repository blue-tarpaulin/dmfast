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

## Release

Rename `## Unreleased` in [CHANGELOG.md](CHANGELOG.md) to the version (e.g. `## 4.4.0`), commit, then push a `v*` tag (e.g. `v4.4.0`). The release notes include that section.
