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
