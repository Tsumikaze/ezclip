# Roadmap & status

Where ezclip stands, what's next, and what still needs checking.
Per-version details are in [CHANGELOG.md](../CHANGELOG.md). Downloads are on the
[Releases](https://github.com/Tsumikaze/ezclip/releases) page.

## Status

| | |
| --- | --- |
| Latest release | **v0.4.0** (2026-11-27) |
| Builds | Every push is built and tested on Windows by GitHub Actions ([build](https://github.com/Tsumikaze/ezclip/actions/workflows/build.yml)); a new version number publishes a release automatically |
| Tested on real Windows | **In review.** The main features are confirmed working with v0.3.2 on the developer's PC (i7-12700K, RX 9070 XT, Windows 10); the less critical checks are still open in [#1](https://github.com/Tsumikaze/ezclip/issues/1) |

### How it has been verified so far
Development happens in a Linux cloud environment, cross-compiling for Windows:
- **Compiles** for Windows (MinGW locally, MSVC in CI) with no warnings.
- **Unit tests:** replay buffer trimming/extension, game-folder matching, clip-name recognition.
- **MP4 output and trimming:** real H.264, HEVC and AV1 streams (+ AAC) generated with
  ffmpeg were pushed through the same code. Results were checked with ffprobe (duration,
  A/V start times) and decoded with ffmpeg with zero errors, including clips with
  B-frames. The HEVC/AV1 config boxes are byte-identical to ffmpeg's.
- **Auto-cleanup:** checked on a mock folder (age limit, size limit, favourites,
  renamed clips, other recorders' files).
- **UI:** rendered and inspected as screenshots.

On a real PC, the main features (recording, game folders, Copy to Discord,
Shrink for Discord) are confirmed working. The remaining, less critical checks are
tracked in [#1](https://github.com/Tsumikaze/ezclip/issues/1), which stays open until
the owner has gone through them.

## Done

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

