# Contributing

## Build

Private patches live in a submodule (`patches-private`); building needs access to it.

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

## Writing a patch

What the shared `base` crate offers a patch, by task: [WRITING-PATCHES.md](WRITING-PATCHES.md).

Features live in `patches/`, shared infrastructure in `base/`, and the install order in `dmfast/src/features.rs`. 
A feature that changes behaviour in any case is a fix, even if it also makes things faster.

### Rules

- Never hardcode a BYOND value. Read it at install from the matched code, or make it a literal byte in the signature
  at an instruction that uses it.
- Check new patterns against every supported BYOND version.
- Keep signatures short: enough bytes to find and pin what the patch relies on.

### Adding a speed patch

1. Declare it `pub` in its folder's `mod.rs` and list it in `dmfast/src/features.rs`.
2. Output must match stock: `.dmb` files byte-identical, and `DMFAST_VERIFY=1` (for a patch that caches answers)
   reports no mismatches on real codebases.
3. Add rows to [PATCHES.md](PATCHES.md) and [PATCH-FIXES.md](PATCH-FIXES.md).

### Adding a bug fix

1. Declare it `pub` with its tier and list it in `dmfast/src/features.rs`.
2. Add `tests/fixes/<key>.dme` and `.expected`; run them with `py tools/fixtest.py <bin> [key]`.
3. Add a row to [BUGFIXES.md](BUGFIXES.md).

### Dependency guards

Patches that mirror BYOND data (string tables and trees, interns, resource ids, sorted maps) often depend on code
beyond the bytes they hook. 
`base/src/deps/mod.rs` checks, that each function such a patch replaces or whose data it mirrors is a known form, 
that their writable globals have the known number of code references, 
and that some unhooked writers have the known number of call sites. A patch that fails stays off.

The accepted state is `patches/src/deps.rs`: fingerprints (a short prefix, a length, and a hash) and counts, kept by
`tools/depsig.py` from a folder of builds, each `<version>/` holding that
version's `byondcore.dll` and `libbyond.so`:

    py tools/depsig.py <builds>                    # check every build
    py tools/depsig.py <builds> --accept 516.1690  # accept that build's changes

A change fails with an instruction diff and the counts that moved, and nothing is written. Check the patches on that
build (fixtest, `DMFAST_VERIFY=1` on real codebases), then accept it.

## Release

Rename `## Unreleased` in [CHANGELOG.md](CHANGELOG.md) to the version (e.g. `## 4.4.0`), commit, then push a `v*` tag (e.g. `v4.4.0`). The release notes include that section.
