# Contributing

## Build

Most patches live in a private submodule (`patches-private`); building needs access to it. 

We believe that openly publishing certain portions of BYOND internals is not what Lummox wants at the moment. 
 However, there's a description of what each one does - and for the optional bugfixes, a bit more detail on how these were accomplished if one wanted to port these to BYOND.

The main motivation behind this project was to accelerate SS13's ability to create fun, creative, and engaging content.
Being stuck with compiler technology of the early 2000s wasn't forwarding that goal. As of this moment, our patches are server and compiler only.

If you believe you can help improve this tool, [open an access request](https://github.com/blue-tarpaulin/dmfast/issues/new?template=access_request.yml). We'll give access to community members we trust.

```
git clone --recurse-submodules https://github.com/blue-tarpaulin/dmfast.git
```

Windows:

```
rustup target add i686-pc-windows-msvc
cargo build --release -p dmfast-install
```

Output: `target/release/dmfast-install.exe` (self-contained).

Linux (needs 32-bit glibc dev files, e.g. `gcc-multilib`):

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
