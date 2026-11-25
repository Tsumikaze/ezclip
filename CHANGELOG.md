# Changelog

## 0.2.0

- **Game folders.** Each clip is named after the game you're playing and saved
  into its folder, e.g. `E:\videos\Valorant\Valorant 2026-11-25 21-14-03.mp4`.
  Your existing folders are matched automatically (by window title, the
  game's product name, its install folder and its exe). Games without a folder
  get a new one (can be turned off). Saving from the tray or the ezclip window
  still uses the game you were just in.
- **Clips folder path can be typed in** (Settings → Saving), as well as browsed.
- **Copy** button on every clip: puts the file on the clipboard, ready to paste
  into Discord (Ctrl+V). Optional: copy every new clip automatically.
- **Tray icon shows the recorder state:** red dot = recording, grey = starting,
  amber = problem (hover for details).
- **Nothing pops up when you save.** Save notifications are off by default (the
  chirp stays). Errors are always shown.
- **Double-press protection:** taps within 1.5 s count once; pressing the hotkey
  again within 10 s extends the clip you just saved instead of making a second one.
- Clips list includes the game folders, shows each clip's game, and reads
  .mp4/.mkv/.mov. Rename, show in folder and delete moved into a "···" menu
  (also on right-click).

## 0.1.0

First release.

- Replay buffer in the system tray: **Alt + F10** saves the last 30 s of screen + audio as MP4
- Hardware H.264 encoding on the GPU (AMD/NVIDIA/Intel). Frames never touch the CPU
- System audio + optional microphone
- Starts with Windows
- Black, minimal window: clips library with thumbnails, plus instant-apply settings (length, monitor, resolution, fps up to 144, quality, audio)
