# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

ezclip is a Windows-only instant-replay clipper written in Rust. It keeps the last N seconds of screen and audio in RAM, already encoded, and writes an MP4 when a hotkey is pressed. `README.md` is the user-facing doc; `CHANGELOG.md` has the per-version notes.

## Commands

The crate refuses to compile on non-Windows targets (`compile_error!` in `src/main.rs`). On Linux, always pass the Windows target.

```bash
# one-time setup on Linux
rustup target add x86_64-pc-windows-gnu
apt-get install -y gcc-mingw-w64-x86-64

cargo check --target x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu   # -> target/x86_64-pc-windows-gnu/release/ezclip.exe
```

On Windows, `cargo build --release` and `cargo test --release` work directly. A single test runs with `cargo test --release trims_to_window_on_keyframes`.

Unit tests live in `src/recorder/buffer.rs`, `src/game_match.rs` and `src/clipmeta.rs`. These files, plus `src/clipfile.rs` and `src/cleanup.rs`, use no Windows APIs, but they sit in a Windows-only crate, so on Linux:
- `cargo test --target x86_64-pc-windows-gnu --no-run` only checks that the tests compile.
- To run them, include those files with `#[path]` in a scratch crate, with tiny stubs for `crate::win` / `crate::log` where needed.

The same `#[path]` approach can also test `recorder/mux.rs` against real H.264/AAC streams made with ffmpeg, checking the result with ffprobe. CI (`.github/workflows/build.yml`, `windows-latest`) runs `cargo test --release` and the MSVC build on every push.

Debug builds keep a console and echo the log to stderr. Release builds use the `windows` subsystem and only log to `%APPDATA%\ezclip\{recorder,ui}.log`.

## Releases

CI publishes a GitHub release `v<version>` whenever `Cargo.toml`'s `version` has no release yet. The release notes are that version's `## <version>` section of `CHANGELOG.md`. To release, bump the version, add the changelog section, and push to the branch.

Do not push git tags: tag pushes are blocked from cloud sessions, and CI creates the tag itself.

Progress is tracked on GitHub. `docs/ROADMAP.md` holds status, done/next lists, verification notes and known limitations. Planned features and the real-hardware test checklist are GitHub issues. When a version ships, update the roadmap and close its issues with a comment.

## Architecture

### Two processes, one exe (`src/main.rs`)
- `ezclip.exe --background` runs the **recorder** (`src/recorder/`): a hidden Win32 window that owns the tray icon, the global hotkey (`RegisterHotKey`) and the capture pipeline. This is what the HKCU `Run` startup entry launches.
- `ezclip.exe` (or `--ui`) runs the **window** (`src/ui/`, eframe/egui 0.29 with the glow backend). It starts the recorder first if it isn't running. Both roles are single-instance via named mutexes.
- The two processes talk through:
  - `PostMessage` to the recorder window, found by class `EzClipRecorderWindow`, using `WM_EZ_RELOAD`, `WM_EZ_SAVE` and `WM_EZ_QUIT` from `src/win.rs`.
  - `%APPDATA%\ezclip\config.json`: the UI writes it, then posts `WM_EZ_RELOAD`.
  - `%APPDATA%\ezclip\status.json`: the recorder writes it via `StatusSink`, and the UI polls it every second.
- `Config::capture_key()` decides whether a reload restarts the capture pipeline. Restarting clears the buffer; hotkey, folder and length changes don't need a restart.

### Capture pipeline (`src/recorder/`)
- `video.rs` (one thread): DXGI Desktop Duplication → copy into a source texture → D3D11 video processor (BGRA→NV12, scaling, BT.709 limited) → a pool of 8 NV12 textures → the hardware H.264 MFT (`encoder.rs`).
  - Textures go to the encoder zero-copy through `IMFDXGIDeviceManager`.
  - Hardware MFTs are **async**: the loop is driven by `METransformNeedInput` / `METransformHaveOutput` events, and frame pacing happens inside the NeedInput handler.
  - A sync software MFT with CPU readback is only a fallback.
  - The encoder uses a 1 s GOP, no B-frames and low-latency mode, so decode order equals display order.
  - Desktop size or format changes rebuild only the video processor; the encoder's output size stays fixed.
- `audio.rs` (one thread): WASAPI loopback plus an optional mic, polled every 10 ms.
  - Each packet is placed on the timeline by its **QPC capture timestamp** (`Clock`), not by arrival time. This keeps sync when loopback is silent or a device starts late.
  - The mixer emits exactly `elapsed × 48 kHz` samples, padding silence, then encodes to AAC with the Media Foundation AAC MFT.
  - It follows default-device changes.
- **Shared clock:** both threads get the same `start: Instant`. All timestamps are 100 ns ticks since `start` (`buffer::TICKS`).
- `buffer.rs`: `ClipBuffer` holds Annex-B video and raw AAC packets as `Arc<[u8]>`.
  - Trimming drops whole GOPs, and a stream must start on an IDR frame.
  - SPS/PPS are taken from the bitstream.
  - Keep length is `clip_seconds + EXTEND_SECS`, which leaves room for double-press extension.
- `mux.rs`: snapshot → MP4 with the `mp4` crate. There is no re-encode: Annex-B is converted to AVCC, SPS/PPS go into avcC, and the tracks are interleaved by time. Audio timing is by AAC frame index from the first video timestamp.
- `mod.rs`: tray, hotkey, saving and the pipeline lifecycle.
  - Recorder state lives in a `thread_local` `RefCell` reached through `with_state` (`try_borrow_mut`, so re-entrant window messages are safe).
  - Never hold the borrow across modal loops such as `TrackPopupMenu`.
  - Saves are serialized on one saver thread. A clip is written as `.mp4.part` and then renamed.
  - A hotkey press within 1.5 s of the last is ignored. Within 10 s it re-snapshots from the previous clip's start and replaces that file.
  - Worker threads reach the window through `post()` and the `HWND_RAW` global.
- `game.rs`: works out where a clip goes. It tracks the last non-shell foreground window (`SetWinEventHook`), then gathers candidate names: window title, exe `ProductName`/`FileDescription`, store install folder, exe stem.
  - `game_match.rs` (pure logic) fuzzy-matches those names against the clips folder's subfolders.
  - When saving from the tray or the ezclip window, it uses the remembered game.

### Clip files (shared by both processes)
- `clipfile.rs`: lossless trim of H.264/AAC MP4s through the `mp4` crate. Sample times come from the `stts`/`stss` tables (`track.trak`). The start snaps back to a keyframe. Trimming in place drops the reader before replacing the file.
- `clipmeta.rs`: two per-clip flags stored as NTFS alternate data streams (`clip.mp4:ezclip.favorite`, `clip.mp4:ezclip.clip`), so they survive renames in Explorer. Favourites fall back to `%APPDATA%\ezclip\favorites.json` on drives without streams. "Made by ezclip" falls back to the `<name> YYYY-MM-DD HH-MM-SS.mp4` file name pattern.
- `cleanup.rs`: `plan()` is a dry run, also used for the Settings preview; `run()` applies it. It only touches clips `clipmeta::is_ezclip` accepts (the user's clips folder also holds other recorders' videos) and never favourites. The recorder runs it on its saver thread at startup, after each save and on reload.

### UI (`src/ui/`)
- `clips.rs`: scans the clips folder and one level of game subfolders. Thumbnails come from the Windows shell (`IShellItemImageFactory`) on a worker thread, and durations from the `mp4` crate.
- `media.rs`: Media Foundation in the window process. `FrameGrabber` is a worker that decodes single frames (source reader → RGB32) for the trim preview and keeps only the newest request. `shrink` is "Shrink for Discord": source reader (advanced video processing: scale + NV12) → sink writer (hardware H.264 + AAC). It picks resolution and fps from the bitrate budget and retries with a lower bitrate if the file comes out too big.
- `editor.rs`: the trim view (`Page::Editor`). Trim and shrink jobs run on threads and report back over channels drained in `App::pump_jobs`.
- `theme.rs` / `widgets.rs`: the black theme loads Segoe UI from `C:\Windows\Fonts` when available. It provides custom chips, a toggle, cards and `setting_row`.

egui layout pitfalls already hit here:
- `Ui::set_max_width(w)` *grows* the region if `w` is larger than the available width. Clamp it to `available_width()`.
- A widget that overflows inside a `ScrollArea` widens every card on the next frame. `setting_row` therefore uses fixed rects with `new_child`. Inside its right-to-left control area, pass chip options in reverse.
- The default egui fonts lack some glyphs (e.g. `→`, `⋯`) that only render when Segoe UI is present. Prefer ASCII or `·`.

### `windows` crate
The project is pinned to `windows` 0.58. Signatures differ between versions; check `~/.cargo/registry/src/*/windows-0.58.0/src/Windows/Win32/...` before guessing. Required Win32 features are listed explicitly in `Cargo.toml`.

`build.rs` embeds `assets/icon.ico` as resource id 1 via `winresource`. Failing to embed only produces a warning. Regenerate the icon with `tools/make_icon.py`, which mirrors the drawing in `src/icon.rs`. The tray icons, one per recorder state, are drawn at runtime by `icon::rgba_state`.
