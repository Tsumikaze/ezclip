//! The background recorder: a hidden window that owns the tray icon and the
//! global hotkey, and runs the capture pipeline (video + audio threads).
mod audio;
mod buffer;
mod encoder;
mod game;
mod mux;
mod video;

use crate::config::{Config, Status};
use crate::win::*;
use buffer::{ClipBuffer, Snapshot};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::MFStartup;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 10;
const WM_SAVED: u32 = WM_APP + 11;
const WM_STATUS: u32 = WM_APP + 12;
/// Pressing the hotkey again within this many seconds of the last press
/// extends the previous clip instead of creating a new one.
const EXTEND_SECS: u32 = 10;
/// Presses closer together than this are treated as one (accidental double tap).
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1500);

/// The recorder window, so worker threads can poke it.
static HWND_RAW: AtomicIsize = AtomicIsize::new(0);

fn post(msg: u32) {
    let h = HWND_RAW.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as _), msg, WPARAM(0), LPARAM(0));
        }
    }
}
const HOTKEY_ID: i32 = 1;
const TRAY_ID: u32 = 1;
const CMD_OPEN: usize = 1;
const CMD_SAVE: usize = 2;
const CMD_FOLDER: usize = 3;
const CMD_QUIT: usize = 4;

// ------------------------------------------------------------ status file ---

/// Shared status, mirrored to %APPDATA%\ezclip\status.json for the UI.
#[derive(Clone, Default)]
pub struct StatusSink(Arc<Mutex<Status>>);

impl StatusSink {
    fn update(&self, f: impl FnOnce(&mut Status)) {
        let mut s = self.0.lock().unwrap();
        f(&mut s);
        let _ = std::fs::create_dir_all(crate::config::app_dir());
        let _ = std::fs::write(crate::config::status_path(), serde_json::to_vec(&*s).unwrap_or_default());
        drop(s);
        post(WM_STATUS);
    }
    fn get(&self) -> Status {
        self.0.lock().unwrap().clone()
    }
    pub fn video_ok(&self, detail: &str) {
        self.update(|s| {
            s.ok = true;
            s.video = detail.into();
            s.error.clear();
        });
    }
    pub fn video_error(&self, e: &str) {
        self.update(|s| {
            s.ok = false;
            s.error = e.into();
        });
    }
    pub fn audio(&self, detail: &str) {
        self.update(|s| s.audio = detail.into());
    }
    fn hotkey_error(&self, e: Option<String>) {
        self.update(|s| {
            if let Some(e) = e {
                s.error = e;
            } else if s.error.starts_with("Hotkey") {
                s.error.clear();
            }
        });
    }
}

// --------------------------------------------------------------- pipeline ---

struct Pipeline {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Pipeline {
    fn start(cfg: &Config, buf: &Arc<ClipBuffer>, status: &StatusSink) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        buf.clear();
        buf.set_keep_seconds(cfg.clip_seconds + EXTEND_SECS);
        let mut threads = Vec::new();
        {
            let (stop, buf, status) = (stop.clone(), buf.clone(), status.clone());
            let p = video::VideoParams { monitor: cfg.monitor, fps: cfg.fps, resolution: cfg.resolution, quality: cfg.quality };
            threads.push(
                std::thread::Builder::new()
                    .name("video".into())
                    .spawn(move || video::thread_main(p, start, stop, buf, status))
                    .unwrap(),
            );
        }
        if cfg.system_audio || cfg.microphone {
            let (stop, buf, status) = (stop.clone(), buf.clone(), status.clone());
            let (sys, mic) = (cfg.system_audio, cfg.microphone);
            threads.push(
                std::thread::Builder::new()
                    .name("audio".into())
                    .spawn(move || audio::thread_main(sys, mic, start, stop, buf, status))
                    .unwrap(),
            );
        } else {
            status.audio("Off");
        }
        Self { stop, threads }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        // Threads notice the flag within one frame. If a driver ever wedges
        // one, give up after 3 s rather than freezing the tray/hotkey.
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        for t in self.threads {
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            } else {
                crate::log::write("a capture thread did not stop in time; detaching it");
            }
        }
    }
}

// ------------------------------------------------------------------ state ---

struct State {
    hwnd: HWND,
    cfg: Config,
    buf: Arc<ClipBuffer>,
    status: StatusSink,
    pipeline: Option<Pipeline>,
    icons: [HICON; 3],
    tray_state: crate::icon::State,
    taskbar_created: u32,
    saver: Sender<SaveJob>,
    last_press: Option<Instant>,
    last_save: Option<LastSave>,
}

struct LastSave {
    at: Instant,
    start_pts: i64,
    path: PathBuf,
}

struct SaveJob {
    snap: Snapshot,
    path: PathBuf,
    extended: bool,
}

struct Saved {
    path: PathBuf,
    secs: f64,
    extended: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Results from save threads, picked up on the UI thread via WM_SAVED.
static SAVE_RESULTS: Mutex<Vec<Result<Saved, String>>> = Mutex::new(Vec::new());

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn run() -> Result<(), String> {
    let Some(_mutex) = single_instance("Local\\EzClipRecorderMutex") else {
        return Ok(()); // already running
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        MFStartup(encoder::MF_VERSION, 0).map_err(|e| format!("Media Foundation unavailable: {e}"))?;

        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: RECORDER_CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            RECORDER_CLASS,
            w!("ezclip recorder"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinst,
            None,
        )
        .map_err(|e| e.to_string())?;

        HWND_RAW.store(hwnd.0 as isize, Ordering::Relaxed);
        let cfg = Config::load();
        let status = StatusSink::default();
        let buf = Arc::new(ClipBuffer::default());
        let icons = [
            state_icon(crate::icon::State::Recording),
            state_icon(crate::icon::State::Starting),
            state_icon(crate::icon::State::Problem),
        ];
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        game::start_tracking();
        let saver = start_saver();
        let pipeline = Pipeline::start(&cfg, &buf, &status);
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                hwnd,
                cfg,
                buf,
                status,
                pipeline: Some(pipeline),
                icons,
                tray_state: crate::icon::State::Starting,
                taskbar_created,
                saver,
                last_press: None,
                last_save: None,
            });
        });
        with_state(|s| {
            add_tray(s);
            register_hotkey(s);
        });
        crate::log::write("recorder running");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Shutdown
        let st = STATE.with(|s| s.borrow_mut().take());
        if let Some(mut st) = st {
            remove_tray(&st);
            let _ = UnregisterHotKey(st.hwnd, HOTKEY_ID);
            if let Some(p) = st.pipeline.take() {
                p.stop();
            }
        }
        let _ = std::fs::remove_file(crate::config::status_path());
    }
    Ok(())
}

/// Tray icon for a recorder state, drawn at the system's small-icon size.
unsafe fn state_icon(state: crate::icon::State) -> HICON {
    let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64) as u32;
    let rgba = crate::icon::rgba_state(size, state);
    let mut bgra = Vec::with_capacity(rgba.len());
    for p in rgba.chunks(4) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let color = CreateBitmap(size as i32, size as i32, 1, 32, Some(bgra.as_ptr() as _));
    let mask_bits = vec![0u8; (size * size / 8) as usize];
    let mask = CreateBitmap(size as i32, size as i32, 1, 1, Some(mask_bits.as_ptr() as _));
    let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info).unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    let _ = DeleteObject(color);
    let _ = DeleteObject(mask);
    icon
}

fn icon_for(s: &State) -> HICON {
    match s.tray_state {
        crate::icon::State::Recording => s.icons[0],
        crate::icon::State::Starting => s.icons[1],
        crate::icon::State::Problem => s.icons[2],
    }
}

/// Recompute the tray state from the pipeline status and refresh the icon.
fn refresh_tray(s: &mut State) {
    use crate::icon::State as T;
    let st = s.status.get();
    s.tray_state = if !st.error.is_empty() {
        T::Problem
    } else if st.ok {
        T::Recording
    } else {
        T::Starting
    };
    let mut nid = base_nid(s);
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = icon_for(s);
    copy_wstr(&mut nid.szTip, &tooltip(s));
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn copy_wstr(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn base_nid(s: &State) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: s.hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

fn tooltip(s: &State) -> String {
    use crate::icon::State as T;
    match s.tray_state {
        T::Recording => format!("ezclip · recording\n{} saves the last {} s", s.cfg.hotkey.label(), s.cfg.clip_seconds),
        T::Starting => "ezclip · starting…".to_string(),
        T::Problem => {
            let e = s.status.get().error;
            let short: String = e.chars().take(100).collect();
            format!("ezclip · problem\n{short}")
        }
    }
}

fn add_tray(s: &State) {
    let mut nid = base_nid(s);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon_for(s);
    copy_wstr(&mut nid.szTip, &tooltip(s));
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray(s: &State) {
    let nid = base_nid(s);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn notify(s: &State, title: &str, text: &str) {
    let mut nid = base_nid(s);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_USER | NIIF_NOSOUND | NIIF_LARGE_ICON;
    nid.hBalloonIcon = s.icons[0];
    copy_wstr(&mut nid.szInfoTitle, title);
    copy_wstr(&mut nid.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn register_hotkey(s: &State) {
    unsafe {
        let _ = UnregisterHotKey(s.hwnd, HOTKEY_ID);
        let hk = s.cfg.hotkey;
        let mut m = MOD_NOREPEAT;
        if hk.ctrl {
            m |= MOD_CONTROL;
        }
        if hk.alt {
            m |= MOD_ALT;
        }
        if hk.shift {
            m |= MOD_SHIFT;
        }
        if RegisterHotKey(s.hwnd, HOTKEY_ID, m, hk.vk).is_ok() {
            s.status.hotkey_error(None);
        } else {
            let e = format!("Hotkey {} is used by another app — pick a different one in Settings", hk.label());
            crate::log::write(&e);
            s.status.hotkey_error(Some(e));
        }
    }
}

/// A short, quiet two-tone chirp generated once as an in-memory WAV.
fn chirp_wav() -> &'static [u8] {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| {
        let rate = 44_100u32;
        let mut pcm: Vec<i16> = Vec::new();
        for (freq, ms) in [(880.0f32, 70u32), (1318.5f32, 90u32)] {
            let n = rate * ms / 1000;
            for i in 0..n {
                let t = i as f32 / rate as f32;
                let env = (i as f32 / 300.0).min(1.0) * ((n - i) as f32 / (n as f32 * 0.6)).min(1.0);
                pcm.push(((t * freq * std::f32::consts::TAU).sin() * env * 0.22 * 32767.0) as i16);
            }
        }
        let data_len = (pcm.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    })
}

fn play_chirp() {
    unsafe {
        let _ = PlaySoundW(PCWSTR(chirp_wav().as_ptr() as _), HMODULE::default(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

/// One thread writes clips, in order, so an "extend" always lands after the
/// clip it replaces.
fn start_saver() -> Sender<SaveJob> {
    let (tx, rx) = channel::<SaveJob>();
    std::thread::Builder::new()
        .name("saver".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                let result = write_clip(&job);
                match &result {
                    Ok(s) => crate::log::write(&format!(
                        "{} {} ({:.1} s)",
                        if s.extended { "extended" } else { "saved" },
                        s.path.display(),
                        s.secs
                    )),
                    Err(e) => crate::log::write(&format!("save failed: {e}")),
                }
                SAVE_RESULTS.lock().unwrap().push(result);
                post(WM_SAVED);
            }
        })
        .unwrap();
    tx
}

fn write_clip(job: &SaveJob) -> Result<Saved, String> {
    let dir = job.path.parent().ok_or("bad path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    // Write under a temp name so nothing ever sees a half-written file;
    // the rename also atomically replaces the old clip when extending.
    let tmp = job.path.with_extension("mp4.part");
    let secs = mux::write_mp4(&tmp, &job.snap).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    if job.extended {
        let _ = std::fs::remove_file(&job.path);
    }
    std::fs::rename(&tmp, &job.path).map_err(|e| e.to_string())?;
    Ok(Saved { path: job.path.clone(), secs, extended: job.extended })
}

fn save_clip(s: &mut State) {
    let now = Instant::now();
    if s.last_press.is_some_and(|t| now.duration_since(t) < DEBOUNCE) {
        return; // accidental double tap
    }
    s.last_press = Some(now);

    // Pressed again shortly after a save: make that clip longer instead.
    if let Some(last) = &mut s.last_save {
        if now.duration_since(last.at) < std::time::Duration::from_secs(EXTEND_SECS as u64) {
            if let Some(snap) = s.buf.snapshot_from(last.start_pts) {
                last.at = now;
                let _ = s.saver.send(SaveJob { snap, path: last.path.clone(), extended: true });
                return;
            }
        }
    }

    let Some(snap) = s.buf.snapshot(s.cfg.clip_seconds) else {
        notify(s, "Nothing to save yet", "ezclip is still starting up — try again in a moment.");
        return;
    };
    let path = game::clip_path(&s.cfg);
    s.last_save = Some(LastSave { at: now, start_pts: snap.video[0].pts, path: path.clone() });
    let _ = s.saver.send(SaveJob { snap, path, extended: false });
}

fn on_saved(s: &State) {
    let results: Vec<_> = SAVE_RESULTS.lock().unwrap().drain(..).collect();
    for r in results {
        match r {
            Ok(saved) => {
                if s.cfg.sound_on_save {
                    play_chirp();
                }
                if s.cfg.copy_after_save {
                    copy_files_to_clipboard(&[&saved.path]);
                }
                if s.cfg.notify_on_save {
                    let name = saved.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let title = if saved.extended {
                        format!("Clip extended · {:.0} s", saved.secs)
                    } else {
                        format!("Clip saved · {:.0} s", saved.secs)
                    };
                    notify(s, &title, &name);
                }
            }
            // Failures are always shown, even with notifications off.
            Err(e) => notify(s, "Couldn't save clip", &e),
        }
    }
}

fn reload(s: &mut State) {
    let new = Config::load();
    let restart = new.capture_key() != s.cfg.capture_key();
    let hotkey_changed = new.hotkey != s.cfg.hotkey;
    s.buf.set_keep_seconds(new.clip_seconds + EXTEND_SECS);
    s.cfg = new;
    if restart {
        crate::log::write("capture settings changed, restarting pipeline");
        if let Some(p) = s.pipeline.take() {
            p.stop();
        }
        s.pipeline = Some(Pipeline::start(&s.cfg, &s.buf, &s.status));
    }
    if hotkey_changed {
        register_hotkey(s);
    }
    refresh_tray(s);
}

fn show_menu(hwnd: HWND, hotkey: &str) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let save = crate::win::wide(&format!("Save clip\t{hotkey}"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN, w!("Open ezclip"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_SAVE, PCWSTR(save.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, CMD_FOLDER, w!("Open clips folder"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, CMD_OPEN as u32, 0);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}

fn run_command(cmd: usize) {
    match cmd {
        CMD_OPEN => spawn_self("--ui"),
        CMD_SAVE => {
            with_state(save_clip);
        }
        CMD_FOLDER => {
            if let Some(dir) = with_state(|s| s.cfg.clips_dir.clone()) {
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
        }
        CMD_QUIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => {
            with_state(save_clip);
            LRESULT(0)
        }
        WM_STATUS => {
            with_state(refresh_tray);
            LRESULT(0)
        }
        WM_SAVED => {
            with_state(|s| on_saved(s));
            LRESULT(0)
        }
        WM_EZ_RELOAD => {
            with_state(reload);
            LRESULT(0)
        }
        WM_EZ_SAVE => {
            with_state(save_clip);
            LRESULT(0)
        }
        WM_EZ_QUIT => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_TRAY => {
            match (lp.0 as u32) & 0xFFFF {
                WM_LBUTTONUP | NIN_BALLOONUSERCLICK => spawn_self("--ui"),
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    // Don't hold the state borrow across the modal menu loop.
                    let hk = with_state(|s| s.cfg.hotkey.label()).unwrap_or_default();
                    let cmd = show_menu(hwnd, &hk);
                    run_command(cmd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_ENDSESSION => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => {
            // Explorer restarted: put the tray icon back.
            let tc = with_state(|s| s.taskbar_created).unwrap_or(0);
            if tc != 0 && msg == tc {
                with_state(|s| {
                    add_tray(s);
                    refresh_tray(s);
                });
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}
