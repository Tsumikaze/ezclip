# Roadmap & status

Where ezclip stands, what's next, and what still needs checking.
Per-version details are in [CHANGELOG.md](../CHANGELOG.md). Downloads are on the
[Releases](https://github.com/Tsumikaze/ezclip/releases) page.

## Status

| | |
| --- | --- |
| Latest release | **v0.3.2** (2026-11-26) |
| Builds | Every push is built and tested on Windows by GitHub Actions ([build](https://github.com/Tsumikaze/ezclip/actions/workflows/build.yml)); a new version number publishes a release automatically |
| Tested on real Windows | **In review.** The main features are confirmed working with v0.3.2 on the developer's PC (i7-12700K, RX 9070 XT, Windows 10); the less critical checks are still open in [#1](https://github.com/Tsumikaze/ezclip/issues/1) |

### How it has been verified so far
Development happens in a Linux cloud environment, cross-compiling for Windows:
- **Compiles** for Windows (MinGW locally, MSVC in CI) with no warnings.
- **Unit tests:** replay buffer trimming/extension, game-folder matching, clip-name recognition.
- **MP4 output and trimming:** real H.264/AAC streams generated with ffmpeg were pushed
  through the same code. Results were checked with ffprobe (duration, A/V start times)
  and decoded with ffmpeg with zero errors, including clips with B-frames.
- **Auto-cleanup:** checked on a mock folder (age limit, size limit, favourites,
  renamed clips, other recorders' files).
- **UI:** rendered and inspected as screenshots.

On a real PC, the main features (recording, game folders, Copy to Discord,
Shrink for Discord) are confirmed working. The remaining, less critical checks are
tracked in [#1](https://github.com/Tsumikaze/ezclip/issues/1), which stays open until
the owner has gone through them.

## Done

### v0.3.2
- [x] Any Discord shrink size: presets plus a custom MB value; per-clip *Shrink to size*

### v0.3.1
- [x] Fix: "Show in folder" opened Documents for paths with spaces
- [x] Copy shrinks clips over the Discord limit first (reuses an existing small copy)
- [x] Brighter favourite star; version shown in the window

### v0.3.0
- [x] Lossless, instant trim editor
- [x] Favourites (stored with the file, survive renames)
- [x] Optional auto-cleanup by size / age (ezclip's clips only, favourites kept, preview first)
- [x] Shrink for Discord (GPU re-encode to a size limit, auto-copy)
- [x] Search, game filter, sort, favourites filter

### v0.2.0
- [x] Clips named after the game and filed into matching folders (e.g. `E:\videos\Valorant`)
- [x] Copy a clip to paste into Discord
- [x] Tray icon shows recording state; nothing visual on save
- [x] Double-press: tap protection, and pressing again within 10 s extends the clip

### v0.1.0
- [x] Replay buffer in the tray, hotkey save, GPU (hardware) H.264 encoding, system audio + mic
- [x] Starts with Windows; black, minimal window with clips library and settings

## Next: v0.4.0 (planned 2026-11-27, 9:00 CET)
- [ ] AV1 / HEVC encoding option: [#2](https://github.com/Tsumikaze/ezclip/issues/2)
- [ ] Hover a thumbnail to preview the clip: [#3](https://github.com/Tsumikaze/ezclip/issues/3)
- [ ] Save sound: volume and choice of sounds: [#4](https://github.com/Tsumikaze/ezclip/issues/4)

## Ideas not scheduled
- Separate audio tracks (game / mic) and volume sliders
- Draw the mouse cursor into clips
- "Only record while a game is running" option
- Second hotkey for a longer clip

## Known limitations
- The mouse cursor isn't drawn into clips.
- The microphone is mixed into the same track as system audio.
- HDR desktops may look washed out.
- Trim only works on H.264 MP4s (most clips; other formats show a message).
- Windows only (by design).

## Working on ezclip
Build commands, architecture notes and gotchas are in [CLAUDE.md](../CLAUDE.md).
The release process: bump `version` in `Cargo.toml`, add a `## <version>` section to
`CHANGELOG.md`, and push. CI builds, tests and publishes the release.
