# Changelog

## 0.6.0

- **ezclip is now Shear**, with a new logo: an open pair of scissors. The app icon is
  white with a black outline; the tray icon no longer changes colour (hover it for the
  status).
- **Your install moves over by itself** the first time Shear runs after updating:
  - `ezclip.exe` becomes `shear.exe` in the same folder.
  - Your settings move from `%APPDATA%\ezclip` to `%APPDATA%\shear`.
  - "Start with Windows" is re-registered as Shear.
  - Pinned shortcuts to the old `ezclip.exe` need re-pinning.
- The Update button keeps working through the repository rename to `Tsumikaze/shear`.
  Clips saved by ezclip are still recognised by auto-cleanup.
- The app icon is now generated from the same code as the in-app logo at build time.

## 0.5.0

- **Choose your microphone** (Settings → Audio → *Microphone device*). Until now
  Shear always used Windows' *default* recording device, which often isn't the mic
  you talk into (e.g. a webcam mic, or a headset that's only the default for calls).
  If the chosen mic is unplugged, Shear uses the default and switches back when it
  returns.
- **Mic volume** (0–300 %), applied live without restarting the replay buffer, with a
  **live level meter**: talk and watch the bar to see that your mic is picked up.
- **Clear microphone problems:** the sidebar and Settings now say *why* the mic isn't
  recording: "blocked by Windows privacy settings" (with a button to open them),
  "device not found", or "silent: muted, or the wrong device?".
- **In-app updates:** Settings → General → *Updates* checks GitHub for a newer
  version, and a green **Update to vX** button appears in the sidebar when one is
  out. One click downloads it, checks it against GitHub's SHA-256 checksum, swaps
  it in and restarts Shear. If Shear lives in a protected folder (e.g. Program
  Files), Windows asks for admin rights once. There's an automatic check when the
  window opens, which can be turned off. Nothing runs in the background.
  - This is the last version you need to install by hand: from here on, use the
    Update button.

## 0.4.2

- **Trim now works on HEVC and AV1 clips too.** In fact it works on any MP4, whatever
  codec or recorder made it: clips with B-frames (e.g. from OBS) and multiple audio
  tracks are kept intact. It's still lossless and instant. The kept frames and the
  codec description are copied over unchanged.
- The trim editor explains when Windows can't show a preview of a clip (HEVC/AV1
  without Microsoft's video extensions) instead of showing a black box. Trimming
  still works in that case.
- Under the hood: Shear now reads and writes MP4 files entirely with its own code
  (no third-party MP4 library), shared by saving and trimming.

## 0.4.1

- **Play clips inside the trim editor**, with picture and sound.
  - The round **Play / Pause** button, clicking the picture, or **Space** plays
    from the red playhead. The playhead follows along.
  - **Play selection** plays exactly what the trimmed clip will contain, from
    the start handle to the end handle.
  - Touching the timeline pauses, so you can play, pause, nudge a handle and
    play again.

## 0.4.0

- **AV1 and HEVC recording** (Settings → Video → *Codec*). On GPUs with an AV1 or
  HEVC hardware encoder (like the RX 9070 XT), clips are about 35 % smaller at the same
  quality, and the replay buffer uses less RAM. H.264 stays the default because it plays
  everywhere. If the chosen encoder isn't available, Shear records H.264 and says so
  in the sidebar.
  - Playing AV1/HEVC clips in Windows' own apps (and their thumbnails) needs
    Microsoft's free *AV1 Video Extension* / the *HEVC Video Extensions*. VLC, mpv,
    browsers and Discord play them without that.
  - *Trim* works on H.264 clips only for now (it says so). *Shrink for Discord* works
    on all of them and always outputs H.264.
- **Hover to preview:** move the mouse across a clip's thumbnail to scrub through it.
  The time badge shows where you are. Click still plays it.
- **Save sound:** choose *Chirp*, *Click*, *Bell* or *Rise*, and set the volume
  (Settings → Saving). Each plays a preview when picked. Still nothing visual on save.
- Saved clips now carry a colour tag (BT.709), so players show the colours exactly as
  recorded.
- Under the hood: a new built-in MP4 writer (H.264 / HEVC / AV1 + AAC). It was
  checked against real streams with ffmpeg: zero decode errors, and codec config
  boxes byte-identical to ffmpeg's own.

## 0.3.2

- **Choose any size for Discord shrinking.** Settings → Sharing keeps the quick
  presets (10 MB Free, 50 MB Nitro Basic, 500 MB Nitro) and adds a *Custom size*
  field for any value, e.g. 25 MB. This is the size *Copy* fits clips to.
- **One-off sizes per clip:** ··· → *Shrink to size* → 10 / 25 / 50 / 100 / 250 / 500 MB
  (plus your custom size), without changing the setting.
- Shrunk copies are now named by size (`<name> (25 MB).mp4`), so different sizes
  of the same clip don't overwrite each other.

## 0.3.1

Fixes from the first real-hardware test ([#1](https://github.com/Tsumikaze/shear/issues/1)):

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
  snaps to the nearest keyframe (every second for Shear's clips). Save a trimmed copy,
  or replace the original. **I** / **O** set start / end at the playhead.
- **Favourites:** the ☆ on each clip. Favourites are never removed by auto-cleanup.
  The mark is stored with the file, so it survives renaming it in Explorer.
- **Auto-cleanup** (Settings → Cleanup, off by default): keep clips under a size
  limit (25–500 GB) and/or delete clips older than 7 days – 1 year, oldest first.
  Only clips made by Shear are ever touched, so older videos from other
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
  get a new one (can be turned off). Saving from the tray or the Shear window
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
