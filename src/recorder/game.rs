//! Figures out which game (or app) is in front when a clip is saved, and
//! where that clip should go.
use crate::config::Config;
use crate::game_match::{best_match, sanitize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Game {
    /// Name used for new folders and file names.
    pub display: String,
    /// Everything we know it by, for matching existing folders.
    pub candidates: Vec<String>,
}

/// Last foreground window that wasn't shear, the taskbar or the desktop.
static LAST_APP: AtomicIsize = AtomicIsize::new(0);

/// Not games: saving while these are in front counts as "Desktop".
const SHELL_EXES: &[&str] = &[
    "explorer",
    "shear",
    "searchhost",
    "searchapp",
    "shellexperiencehost",
    "startmenuexperiencehost",
    "textinputhost",
    "lockapp",
    "taskmgr",
];
const SHELL_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Progman",
    "WorkerW",
];
/// Install-path folder names that say nothing about the game.
const GENERIC_DIRS: &[&str] = &[
    "program files", "program files (x86)", "steamapps", "common", "steam", "games", "game", "bin", "binaries",
    "win64", "win32", "x64", "x86", "windows", "windowsapps", "retail", "live", "client", "launcher", "epic games",
    "riot games", "ea games", "ubisoft", "ubisoft game launcher", "gog games", "xboxgames", "content", "users",
    "appdata", "local", "roaming", "programs", "shipping", "build", "release",
];

/// Start following the foreground window (call once on the recorder's UI thread).
pub fn start_tracking() {
    unsafe {
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
        let fg = GetForegroundWindow();
        on_foreground(HWINEVENTHOOK::default(), 0, fg, 0, 0, 0, 0);
    }
}

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    if !hwnd.is_invalid() && classify(hwnd) == Kind::App {
        LAST_APP.store(hwnd.0 as isize, Ordering::Relaxed);
    }
}

#[derive(PartialEq)]
enum Kind {
    App,
    /// shear itself or the taskbar/tray: look at what was in front before.
    Ours,
    Desktop,
}

fn classify(hwnd: HWND) -> Kind {
    let class = window_class(hwnd);
    if class.starts_with("Shell_") || class == "NotifyIconOverflowWindow" || class == "TopLevelWindowForOverflowXamlIsland" {
        return Kind::Ours;
    }
    if SHELL_CLASSES.contains(&class.as_str()) {
        return Kind::Desktop;
    }
    match process_path(hwnd) {
        Some(p) => {
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            if stem == "shear" {
                Kind::Ours
            } else if SHELL_EXES.contains(&stem.as_str()) {
                Kind::Desktop
            } else {
                Kind::App
            }
        }
        None => Kind::App,
    }
}

/// The game in front right now (None = desktop / nothing game-like).
pub fn current() -> Option<Game> {
    unsafe {
        let mut hwnd = GetForegroundWindow();
        match classify(hwnd) {
            Kind::App => {}
            Kind::Desktop => return None,
            Kind::Ours => {
                // Saved from the tray or shear's window: use the app that was
                // in front before, if it still exists.
                hwnd = HWND(LAST_APP.load(Ordering::Relaxed) as _);
                if hwnd.is_invalid() || !IsWindow(hwnd).as_bool() {
                    return None;
                }
            }
        }
        describe(hwnd)
    }
}

fn describe(hwnd: HWND) -> Option<Game> {
    let title = window_title(hwnd);
    let path = process_path(hwnd);
    let mut store_dir = None;
    let mut product = None;
    let mut description = None;
    let mut exe = None;
    let mut dirs = Vec::new();

    if let Some(p) = &path {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if !stem.eq_ignore_ascii_case("applicationframehost") {
            exe = Some(stem);
            product = version_string(p, "ProductName").filter(|s| !is_generic_product(s));
            description = version_string(p, "FileDescription").filter(|s| !is_generic_product(s));
        }
        // Folder names along the install path ("…\steamapps\common\Apex Legends\r5apex.exe").
        let comps: Vec<String> = p.parent().into_iter().flat_map(|d| d.components()).map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        for (i, c) in comps.iter().enumerate() {
            let lc = c.to_lowercase();
            let prev = if i > 0 { comps[i - 1].to_lowercase() } else { String::new() };
            if store_dir.is_none()
                && (prev == "common" || prev == "epic games" || prev == "riot games" || prev == "gog games" || prev == "xboxgames" || prev == "games" || prev == "ea games")
                && !GENERIC_DIRS.contains(&lc.as_str())
            {
                store_dir = Some(c.clone());
            }
        }
        for c in comps.iter().rev().take(6) {
            let lc = c.to_lowercase();
            if !GENERIC_DIRS.contains(&lc.as_str()) && !lc.ends_with(':') && !lc.ends_with(":\\") && c.len() > 2 {
                dirs.push(c.clone());
            }
        }
    }

    let title_ok = (2..=48).contains(&title.chars().count()).then(|| title.clone());
    let display = [store_dir.clone(), product.clone(), title_ok.clone(), description.clone(), exe.clone()]
        .into_iter()
        .flatten()
        .map(|s| sanitize(&s))
        .find(|s| !s.is_empty())?;

    let mut candidates: Vec<String> = [Some(title), store_dir, product, description, exe].into_iter().flatten().collect();
    candidates.extend(dirs);
    candidates.retain(|c| !c.trim().is_empty());
    Some(Game { display, candidates })
}

fn is_generic_product(s: &str) -> bool {
    let l = s.to_lowercase();
    l.is_empty()
        || l.contains("operating system")
        || l == "unrealgame"
        || l == "unreal engine"
        || l.starts_with("bootstrap")
        || l == "java(tm) platform se binary"
        || l.starts_with("openjdk")
}

/// Where a new clip should be written.
pub fn clip_path(cfg: &Config) -> PathBuf {
    let root = cfg.clips_dir.clone();
    let game = current();
    let (dir, label) = match game {
        Some(g) if cfg.game_folders => {
            let folders = subfolders(&root);
            match best_match(&folders, &g.candidates) {
                Some(i) => (root.join(&folders[i]), folders[i].clone()),
                None if cfg.new_game_folders => (root.join(&g.display), g.display),
                None => (root.clone(), g.display),
            }
        }
        Some(g) => (root.clone(), g.display),
        None => (root.clone(), "Desktop".to_string()),
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let mut path = dir.join(format!("{label} {stamp}.mp4"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{label} {stamp} ({n}).mp4"));
        n += 1;
    }
    path
}

fn subfolders(root: &Path) -> Vec<String> {
    std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------ win32 bits ---

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string()
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn process_path(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Read a string from the exe's version resource (ProductName, FileDescription).
fn version_string(path: &Path, key: &str) -> Option<String> {
    unsafe {
        let wpath = crate::win::wide(&path.to_string_lossy());
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), 0, size, data.as_mut_ptr() as _).ok()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let tq = crate::win::wide("\\VarFileInfo\\Translation");
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(data.as_ptr() as _, PCWSTR(tq.as_ptr()), &mut ptr, &mut len).as_bool() && len >= 4 {
            let p = ptr as *const u16;
            for i in 0..(len as usize / 4) {
                langs.push((*p.add(i * 2), *p.add(i * 2 + 1)));
            }
        }
        langs.push((0x0409, 0x04B0));
        langs.push((0x0409, 0x04E4));
        for (lang, cp) in langs {
            let q = crate::win::wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"));
            if VerQueryValueW(data.as_ptr() as _, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool() && len > 1 {
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len as usize - 1));
                let s = s.trim_end_matches('\0').trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}
