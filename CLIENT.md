# DreamSeeker patches

Fixes for DreamSeeker, BYOND's client. Their code is in [a separate repo](./CONTRIBUTING.md#build).

For verification details, see: [seeker/src/validate.rs](seeker/src/validate.rs)

These are off by default. Turn them on when installing:

```
dmfast-install --client "C:\Program Files (x86)\BYOND"
```

That applies the `safe` ones. Pick others by [tier](BUGFIXES.md) or key, like bug fixes: `--client extended`. Your choice is saved in `dmfast.ini` and applied again by later installs; `--client none` puts DreamSeeker back to stock.

## Fixes

| Fix | Key | Tier | What changes | Bug report |
|---|---|---|---|---|
| dmfast banner | `version` | safe | Options and Messages says the dmfast version | - |
| Particle drawing | `particles` | safe | A set of more than 10,922 icon particles of one icon no longer writes past the end of BYOND's index buffer, and the depth buffer covers the whole render target. | - |
| `preload_rsc` downloads | `rscdownload` | safe | Fast `preload_rsc` downloads are no longer cancelled midway. DreamDaemon's `world.Export()` has the same bug: the `httpexport` [bug fix](BUGFIXES.md). | [2992012](https://www.byond.com/forum/post/2992012) |
| More sounds at once | `voices` | safe | Mixes up to 128 sounds at once instead of 32, so many effects no longer silence background sounds. | - |
| Icon chains | `chainmem` | safe | A runtime icon rebuilt from its own icon many times (a chain) is drawn 13 links at a time, so DreamSeeker no longer holds every link in memory at once. The server's `iconchain` [bug fix](BUGFIXES.md) keeps chains short. | - |
| Stray DreamSeekers | `commandlinks` | safe | A DreamSeeker started only for a page's `byond://winset or get` link (one that escaped its page) exits at once instead of opening the pager and staying open and idle. | [2914968](https://www.byond.com/forum/post/2914968) |
| Echo in stereo | `echostereo` | safe | A sound with `echo` or `environment` but no position keeps its stereo, instead of being downmixed to mono. | [2963783](https://www.byond.com/forum/post/2963783) |
| Smooth pitch changes | `pitchretune` | safe | Updating a playing sound's `pitch` (`SOUND_UPDATE`) retunes its pitch filters instead of restarting them, so a changing pitch no longer stutters. | [2953774](https://www.byond.com/forum/post/2953774) |
| Faster verb panels | `objsearch` | safe | Finding what a verb can apply to looks for the next used slot of the client's obj table by checking 16 slots at once (most slots are empty). | - |
| Hidden maps | `hiddenmap` | safe | A map control that isn't showing is drawn when it shows again instead of on every change. Windows holds up drawing to a hidden window for 30 ms, so its animated icons made movement on the other maps stutter. | [2992242](https://www.byond.com/forum/post/2992242) |
| Sound priority | `soundpriority` | extended | A higher priority (per the ref) keeps its channel instead of being the first silenced. Sounds left at the default 0 are now the first silenced. | - |
