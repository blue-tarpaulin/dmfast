# Changelog

Write changes under `## Unreleased`. Before tagging a release, rename that heading to the version; the release notes include that section's text as is.

## 6.0.1

- **Fixed:** on Linux, the `callref` fix broke `call("name")(...)`: every call by name failed with "bad proc".
- `num2sig` moves to the `breaking` tier: `num2text(N, 100)`, used to print ids exactly, became a 100-digit string.
- CI runs regression worlds (`tests/fixes/keep`) and the DM test suite (spacestation13/dm-test-suite) against stock BYOND and dmfast.

## 6.0.0

- **Opt-in bug fixes:** dmfast now carries 24 fixes for BYOND bugs, off unless you turn them on. 
  - For example, `dmfast-install --fixes safe`. `safe` or `extended` is recommended. See [BUGFIXES.md](BUGFIXES.md).
- **Guided install:** double-click the installer (or run it bare in a Linux term) to set it up step by step.

## 5.0.0

- **DreamDaemon is sped up by default.** The installer now patches DreamDaemon too; pass `--no-runtime` to leave it stock. `--runtime` is no longer needed (still accepted).
- **Less memory:** dmfast's allocator now uses mimalloc only for small blocks and the system heap for larger ones: about 100 MB less on a large world load, same speed. `mimalloc-redirect32.dll` is no longer installed; upgrading removes it.
- **Sorted maps:** the `sortmerge` key is gone; `DMFAST_DISABLE=sortmap` now turns off both the merge and the insert patch.
- **Removed:** DreamDaemon's `.dmb` read buffer, which measured no gain. The compiler's `dmbbuf` stays.

## NEXT MAJOR VERSION

- Rename the `resids` patch key to `procids`
