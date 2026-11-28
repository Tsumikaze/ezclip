# Roadmap & status

Where ezclip stands, what's next, and what still needs checking.
Per-version details are in [CHANGELOG.md](../CHANGELOG.md). Downloads are on the
[Releases](https://github.com/Tsumikaze/ezclip/releases) page.

## Status

| | |
| --- | --- |
| Latest release | **v0.5.0** (2026-11-28) |
| Builds | Every push is built and tested on Windows by GitHub Actions ([build](https://github.com/Tsumikaze/ezclip/actions/workflows/build.yml)); a new version number publishes a release automatically |
| Tested on real Windows | **In review.** The main features are confirmed working with v0.3.2 on the developer's PC (i7-12700K, RX 9070 XT, Windows 10); the less critical checks are still open in [#1](https://github.com/Tsumikaze/ezclip/issues/1) |

### How it has been verified so far
Development happens in a Linux cloud environment, cross-compiling for Windows:
- **Compiles** for Windows (MinGW locally, MSVC in CI) with no warnings.
- **Unit tests:** replay buffer trimming/extension, game-folder matching, clip-name recognition.
- **MP4 output and trimming:** real H.264, HEVC and AV1 streams (+ AAC) generated with
  ffmpeg were pushed through the same code. Results were checked with ffprobe (duration,
  A/V start times) and decoded with ffmpeg with zero errors, including clips with
  B-frames. The HEVC/AV1 config boxes are byte-identical to ffmpeg's. Trimming was tested
  on ezclip's own H.264/HEVC/AV1 clips and on ffmpeg-made files, then re-trimmed and
  trimmed in place.
- **Updates:** the release reader and SHA-256 were checked against GitHub's live answer
  and the real v0.4.2 download (checksum matches GitHub's digest).
- **Auto-cleanup:** checked on a mock folder (age limit, size limit, favourites,
  renamed clips, other recorders' files).
- **UI:** rendered and inspected as screenshots.

On a real PC, the main features (recording, game folders, Copy to Discord,
Shrink for Discord) are confirmed working. The remaining, less critical checks are
tracked in [#1](https://github.com/Tsumikaze/ezclip/issues/1), which stays open until
the owner has gone through them.

## Done

### v0.5.0
- [x] Microphone device picker, live mic volume, level meter, and clear mic diagnostics
- [x] In-app updates from GitHub releases (SHA-256 verified, elevates only if needed)

### v0.4.2
- [x] Trim works on every codec and recorder: HEVC and AV1 too, B-frames, multiple audio tracks

### v0.4.1
- [x] Playback (picture + sound) in the trim editor; *Play selection* plays what will be kept

### v0.4.0
- [x] AV1 / HEVC recording option with H.264 fallback, plus a new built-in MP4 writer ([#2](https://github.com/Tsumikaze/ezclip/issues/2))
- [x] Hover a thumbnail to scrub through the clip ([#3](https://github.com/Tsumikaze/ezclip/issues/3))
- [x] Save sound: four sounds and a volume control ([#4](https://github.com/Tsumikaze/ezclip/issues/4))

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

## Next
Nothing scheduled. Pick from the ideas below, or open an issue.

## Ideas not scheduled
- Separate audio tracks (game / mic) and volume sliders
- Draw the mouse cursor into clips
- "Only record while a game is running" option
- Second hotkey for a longer clip

## Known limitations
- The mouse cursor isn't drawn into clips.
- The microphone is mixed into the same track as system audio.
- HDR desktops may look washed out.
- AV1/HEVC clips need Microsoft's video extensions for Windows' own players and for
  ezclip's previews (thumbnails, hover, trim preview and playback). Trimming itself works
  without them; VLC, browsers and Discord play these clips anyway.
- Windows only (by design).

## Working on ezclip
Build commands, architecture notes and gotchas are in [CLAUDE.md](../CLAUDE.md).
The release process: bump `version` in `Cargo.toml`, add a `## <version>` section to
`CHANGELOG.md`, and push. CI builds, tests and publishes the release.
