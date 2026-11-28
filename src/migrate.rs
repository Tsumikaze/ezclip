//! One-time move from the old name (ezclip) to Shear, done automatically the
//! first time Shear runs after updating from ezclip:
//! * `ezclip.exe` is copied to `shear.exe` next to it and Shear relaunches from there,
//! * settings move from `%APPDATA%\ezclip` to `%APPDATA%\shear`,
//! * the "Start with Windows" entry is re-registered under the new name,
//! * a still-running old recorder is stopped, and the old exe is removed.
use std::path::PathBuf;
use windows::core::w;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

const OLD: &str = "ezclip";

/// If this is still `ezclip.exe`, put a `shear.exe` copy next to it and return
/// its path (the caller relaunches it). `None` = carry on as we are.
pub fn exe_name() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let name = me.file_name()?.to_string_lossy().to_lowercase();
    if name != format!("{OLD}.exe") {
        return None;
    }
    let target = me.with_file_name("shear.exe");
    if !target.exists() {
        if let Err(e) = std::fs::copy(&me, &target) {
            // e.g. a folder that needs admin rights: keep running under the old name.
            crate::log::write(&format!("rename: can't create {} ({e}); keeping {}", target.display(), me.display()));
            return None;
        }
    }
    Some(target)
}

pub fn run() {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else { return };

    // Settings: copy once, the old folder stays as a backup.
    let old_dir = appdata.join(OLD);
    let new_dir = crate::config::app_dir();
    if old_dir.join("config.json").exists() && !new_dir.join("config.json").exists() {
        let _ = std::fs::create_dir_all(&new_dir);
        for f in ["config.json", "favorites.json"] {
            let _ = std::fs::copy(old_dir.join(f), new_dir.join(f));
        }
        crate::log::write("rename: moved settings from %APPDATA%\\ezclip");
    }

    // "Start with Windows": replace the old entry with one for this exe.
    if crate::win::remove_legacy_startup() {
        crate::win::set_startup(true);
        crate::log::write("rename: start-with-Windows entry now points to Shear");
    }

    // An ezclip recorder from before the update would hold the hotkey.
    unsafe {
        if let Ok(h) = FindWindowW(w!("EzClipRecorderWindow"), windows::core::PCWSTR::null()) {
            if !h.is_invalid() {
                let _ = PostMessageW(h, crate::win::WM_SHEAR_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }

    // Remove the old exe once the process that was running it has exited.
    if let Ok(me) = std::env::current_exe() {
        let old_exe = me.with_file_name(format!("{OLD}.exe"));
        if old_exe.exists() && old_exe != me {
            std::thread::spawn(move || {
                for _ in 0..30 {
                    if std::fs::remove_file(&old_exe).is_ok() || !old_exe.exists() {
                        crate::log::write("rename: removed ezclip.exe");
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            });
        }
    }
}
