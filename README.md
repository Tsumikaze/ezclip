# ezclip

Fast, simple clipping for Windows. ezclip quietly keeps the last few seconds of
your screen **and audio** in memory. Press a hotkey and they're saved as an MP4.

- **Alt + F10** saves the last 30 seconds (both are configurable)
- Starts with Windows and lives in the system tray
- Uses your GPU's hardware video encoder (AMD, NVIDIA or Intel), so games keep their FPS
- Records system audio (game, music, voice chat) and, optionally, your microphone
- One small `.exe`, no installer, no account, no telemetry

## Install

1. Download `ezclip.exe` (from the GitHub Actions **build** artifact or a release)
   and put it somewhere permanent, e.g. `C:\Program Files\ezclip\ezclip.exe`.
2. Double-click it. The window opens and a ◉ icon appears in the tray.
   That's it: the replay buffer is now running.

ezclip turns on *Start with Windows* the first time it runs. You can switch it off
in **Settings → General**. If you move the exe later, toggle it off and on again.

## Use

| Action | How |
| --- | --- |
| Save a clip | Press **Alt + F10**, or click **Save last 30 s** in the window, or right-click the tray icon → *Save clip* |
| See your clips | Click the tray icon. Click a thumbnail to play it; rename, show in folder or delete (to the Recycle Bin) from the list |
| Change hotkey / length / quality | **Settings**. Changes apply instantly |
| Quit | Right-click the tray icon → *Quit* |

Clips are saved to `Videos\ezclip` by default. A short chirp and a notification
confirm each save (both can be turned off).

## Settings

- **Clip length**: 15 s – 5 min
- **Monitor**: which screen to record
- **Resolution**: Native, 1440p, 1080p, 720p (downscaling happens on the GPU)
- **Frame rate**: 30, 60, 120, 144
- **Quality**: Low / Medium / High / Ultra. The page shows the bitrate and how much RAM the buffer uses.
- **Audio**: system audio and/or microphone (mixed into one track)

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
