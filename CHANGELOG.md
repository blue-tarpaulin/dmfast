# Changelog

Write changes under `## Unreleased`. Before tagging a release, rename that heading to the version; the release notes include that section's text as is.

## Unreleased

- **Opt-in bug fixes:** dmfast can now carry fixes for BYOND bugs, kept apart from the speed patches and off unless turned on with `DMFAST_FIXES`. See [BUGFIXES.md](BUGFIXES.md). With `DMFAST_STATS=1`, the stats line now also shows `fixed=`.

## 5.0.0

- **DreamDaemon is sped up by default.** The installer now patches DreamDaemon too; pass `--no-runtime` to leave it stock. `--runtime` is no longer needed (still accepted).
- **Less memory:** dmfast's allocator now uses mimalloc only for small blocks and the system heap for larger ones: about 100 MB less on a large world load, same speed. `mimalloc-redirect32.dll` is no longer installed; upgrading removes it.
- **Sorted maps:** the `sortmerge` key is gone; `DMFAST_DISABLE=sortmap` now turns off both the merge and the insert patch.
- **Removed:** DreamDaemon's `.dmb` read buffer, which measured no gain. The compiler's `dmbbuf` stays.
- **`patched=` masks renumbered** without gaps, so they differ from earlier versions.
