# Changelog

Write changes under `## 6.0.1`. Before tagging a release, rename that heading to the version; the release notes include that section's text as is.

## 6.0.1

- `num2sig` moves from the `safe` tier to `breaking`: with `num2text(N, 100)`, which codebases use to print ids exactly, it produces 100-digit strings.

## 6.0.0

- **Opt-in bug fixes:** dmfast now carries 24 fixes for BYOND bugs, off unless you turn them on. 
  - For example, `dmfast-install --fixes safe`. `safe` or `extended` is recommended. See [BUGFIXES.md](BUGFIXES.md).
- **Guided install:** double-click the installer (or run it bare in a Linux term) to set it up step by step.

## 5.0.0

- **DreamDaemon is sped up by default.** The installer now patches DreamDaemon too; pass `--no-runtime` to leave it stock. `--runtime` is no longer needed (still accepted).
- **Less memory:** dmfast's allocator now uses mimalloc only for small blocks and the system heap for larger ones: about 100 MB less on a large world load, same speed. `mimalloc-redirect32.dll` is no longer installed; upgrading removes it.
- **Sorted maps:** the `sortmerge` key is gone; `DMFAST_DISABLE=sortmap` now turns off both the merge and the insert patch.
- **Removed:** DreamDaemon's `.dmb` read buffer, which measured no gain. The compiler's `dmbbuf` stays.
