# Changelog

## 0.3.2

- **Choose any size for Discord shrinking.** Settings → Sharing keeps the quick
  presets (10 MB Free, 50 MB Nitro Basic, 500 MB Nitro) and adds a *Custom size*
  field for any value, e.g. 25 MB. This is the size *Copy* fits clips to.
- **One-off sizes per clip:** ··· → *Shrink to size* → 10 / 25 / 50 / 100 / 250 / 500 MB
  (plus your custom size), without changing the setting.
- Shrunk copies are now named by size (`<name> (25 MB).mp4`), so different sizes
  of the same clip don't overwrite each other.

## 0.3.1

Fixes from the first real-hardware test ([#1](https://github.com/Tsumikaze/ezclip/issues/1)):

- **Fixed: "Show in folder" opened Documents** instead of the clip's folder, for clip
  paths containing spaces. It now asks Windows directly to open the folder with the
  clip selected.
- **Copy now fits Discord:** if a clip is over your upload limit (Settings →
  Sharing, default 10 MB), Copy shrinks it first and copies the small version when
  it's ready. An up-to-date small version is reused instantly next time. The
  full-size file is still available under ··· → *Copy original file*. This can be
  switched off in Settings → Sharing.
- **Easier to find favourites:** the ☆ next to *Play* is brighter. Click it to
  favourite a clip (it turns gold), or use ··· → *Add to favourites*.
- **Version shown** next to the logo and in Settings → General, with a link to the releases.

## 0.3.0

- **Trim clips** (··· menu → *Trim…*): a preview and a timeline with start/end
  handles. Trimming is lossless and instant: nothing is re-encoded, and the start
  snaps to the nearest keyframe (every second for ezclip's clips). Save a trimmed copy,
  or replace the original. **I** / **O** set start / end at the playhead.
- **Favourites:** the ☆ on each clip. Favourites are never removed by auto-cleanup.
  The mark is stored with the file, so it survives renaming it in Explorer.
- **Auto-cleanup** (Settings → Cleanup, off by default): keep clips under a size
  limit (25–500 GB) and/or delete clips older than 7 days – 1 year, oldest first.
  Only clips made by ezclip are ever touched, so older videos from other
  recorders in the same folders are safe. Uses the Recycle Bin by default.
  Settings shows exactly what would be removed before you turn it on.
- **Shrink for Discord** (··· menu): re-encodes on the GPU to fit your upload
  limit (10 / 50 / 500 MB, set in Settings → Sharing). It lowers resolution and
  frame rate as needed, then saves `<name> (discord).mp4` next to the clip and
  copies it, ready for Ctrl+V. Clips already under the limit are just copied.
- **Search, sort and filter** the clips list: search by name or game, filter by
  game or favourites, sort by newest / oldest / largest / longest / name. The
  list stays smooth with hundreds of clips.

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
