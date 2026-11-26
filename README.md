# ezclip

[![build](https://github.com/Tsumikaze/ezclip/actions/workflows/build.yml/badge.svg)](https://github.com/Tsumikaze/ezclip/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/Tsumikaze/ezclip)](https://github.com/Tsumikaze/ezclip/releases/latest)

**Download:** [latest release](https://github.com/Tsumikaze/ezclip/releases/latest) ·
**Status & roadmap:** [docs/ROADMAP.md](docs/ROADMAP.md) ·
**Changes:** [CHANGELOG.md](CHANGELOG.md)

Fast, simple clipping for Windows. ezclip quietly keeps the last few seconds of
your screen **and audio** in memory. Press a hotkey and they're saved as an MP4.

- **Alt + F10** saves the last 30 seconds (both are configurable). Press again within 10 s to make that clip longer
- Clips are named after the game and filed into your game folders (e.g. `E:\videos\Valorant`)
- One click copies a clip so you can paste it straight into Discord, or shrinks it to fit Discord's upload limit
- Lossless, instant trimming; favourites; optional auto-cleanup of old clips
- Starts with Windows and lives in the system tray
- Uses your GPU's hardware video encoder (AMD, NVIDIA or Intel), so games keep their FPS
- Records system audio (game, music, voice chat) and, optionally, your microphone
- One small `.exe`, no installer, no account, no telemetry

## Install

1. Download `ezclip.exe` from the [latest release](https://github.com/Tsumikaze/ezclip/releases/latest)
   and put it somewhere permanent, e.g. `C:\Program Files\ezclip\ezclip.exe`.
2. Double-click it. The window opens and a ◉ icon appears in the tray.
   That's it: the replay buffer is now running.

ezclip turns on *Start with Windows* the first time it runs. You can switch it off
in **Settings → General**. If you move the exe later, toggle it off and on again.

## Use

| Action | How |
| --- | --- |
| Save a clip | Press **Alt + F10**, or click **Save last 30 s** in the window, or right-click the tray icon → *Save clip* |
| See your clips | Click the tray icon. Click a thumbnail to play it. **Copy** puts it on the clipboard for Discord. **···** (or right-click) has rename, show in folder and delete (to the Recycle Bin) |
| Make a clip longer | Press the hotkey again within 10 s: the clip you just saved is extended |
| Trim | **···** → *Trim…*: drag the white handles, then *Save as new clip* or *Replace original*. Lossless and instant |
| Send to Discord | **Copy**, then Ctrl+V. If it's too big: **···** → *Shrink for Discord*. The small copy is made and copied for you |
| Keep a clip forever | Click its ☆. Favourites are never auto-deleted |
| Find a clip | Search box, game filter, sort menu and *Favourites* filter above the list |
| Change hotkey / length / quality | **Settings**. Changes apply instantly |
| Quit | Right-click the tray icon → *Quit* |

Clips are saved to `Videos\ezclip` by default. Change it in **Settings → Saving**:
type a path (e.g. `E:\videos`) or browse. With *Sort into game folders* on, each clip
goes into the subfolder matching the game you're in (`E:\videos\Valorant`, `E:\videos\Counter-Strike 2`, ...).
Existing folders are matched automatically. New games get a folder of their own, and
clips from the desktop are saved in the main folder.

A short chirp confirms each save. Nothing pops up on screen, though you can turn
notifications on. The tray icon's dot shows the state: **red** = recording,
**grey** = starting, **amber** = problem (hover it for details).

## Settings

- **Clip length**: 15 s – 5 min
- **Monitor**: which screen to record
- **Resolution**: Native, 1440p, 1080p, 720p (downscaling happens on the GPU)
- **Frame rate**: 30, 60, 120, 144
- **Quality**: Low / Medium / High / Ultra. The page shows the bitrate and how much RAM the buffer uses.
- **Audio**: system audio and/or microphone (mixed into one track)
- **Sharing**: your Discord upload limit (10 / 50 / 500 MB) for *Shrink for Discord*
- **Cleanup** (off by default): keep clips under a size limit and/or delete clips older
  than N days, oldest first. Only clips made by ezclip are touched (older videos from
  other recorders are safe) and favourites are always kept. The page shows what would be
  removed before you switch it on.

## How it works (why it's light)

```
Desktop Duplication (GPU) ─► D3D11 video processor: BGRA→NV12 + scale (GPU)
        ─► hardware H.264 encoder via Media Foundation (GPU, e.g. AMD VCN)
        ─► compressed packets in a RAM ring buffer (last N seconds)
WASAPI loopback + mic ─► 48 kHz mixer (timestamp-aligned) ─► AAC ─► same ring buffer

Hotkey ─► copy buffer ─► MP4 (no re-encoding, ~50 ms for a 30 s clip)
```

- Video frames never leave the GPU. The CPU only paces frames and moves small packets.
- Nothing is written to disk until you save, so there's no constant disk activity.
- The background recorder has no UI framework loaded. The window is a separate
  process that only exists while it's open.
- RAM use is roughly `bitrate × clip length` (about 180 MB for 30 s of 1440p60 on *High*).

## Troubleshooting

- **Settings → General → Logs** opens `%APPDATA%\ezclip`, which contains `recorder.log` and `ui.log`.
- *Hotkey is used by another app*: pick a different one in Settings
  (AMD Adrenalin and GeForce Experience use some Alt+F-keys).
- If the sidebar says *CPU encoder*, Windows couldn't open your GPU's encoder.
  Update the graphics driver.
- Debloated Windows builds must still include Media Foundation, which provides
  the H.264 and AAC encoders. On "N" editions, install the Media Feature Pack.
- The mouse cursor isn't drawn into clips (games normally hide it anyway).

## Build from source

Requires Rust (stable).

```
cargo build --release          # on Windows → target\release\ezclip.exe
```

Cross-compiling from Linux also works:

```
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```

Project layout:

```
src/main.rs            entry: --background = recorder, otherwise opens the window
src/recorder/          tray, hotkey, capture pipeline
  video.rs             desktop duplication, GPU colour conversion, frame pacing
  audio.rs             WASAPI capture + mixer
  encoder.rs           Media Foundation H.264 (hardware) + AAC encoders
  buffer.rs            in-memory replay buffer
  mux.rs               MP4 writer
src/ui/                the window (egui): clips library + settings
```
