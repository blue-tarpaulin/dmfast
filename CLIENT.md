# DreamSeeker patches

Fixes for DreamSeeker, BYOND's client. Their code is in [a separate repo](./CONTRIBUTING.md#build).

For verification details, see: [seeker/src/validate.rs](seeker/src/validate.rs)

These are off by default. Turn them on when installing:

```
dmfast-install --client "C:\Program Files (x86)\BYOND"
```

By default, only the `safe` ones are applied. You can pass `--fixes` to pick others, by [tier](BUGFIXES.md) or key, the same as bug fixes.
E.g. `--client --fixes extended`.

## Fixes

| Fix | Key | Tier | What changes | Bug report |
|---|---|---|---|---|
| dmfast banner | `version` | safe | Options and Messages says the dmfast version | - |
| Particle drawing | `particles` | safe | A set of more than 10,922 icon particles of one icon no longer writes past the end of BYOND's index buffer, and the depth buffer covers the whole render target. | - |
| `preload_rsc` downloads | `rscdownload` | safe | Fast `preload_rsc` downloads are no longer cancelled midway. DreamDaemon's `world.Export()` has the same bug: the `httpexport` [bug fix](BUGFIXES.md). | [2992012](https://www.byond.com/forum/post/2992012) |
