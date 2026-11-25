//! Small Win32 helpers shared by the recorder and the UI.
use std::path::{Path, PathBuf};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const RECORDER_CLASS: PCWSTR = w!("EzClipRecorderWindow");
pub const WM_EZ_RELOAD: u32 = WM_APP + 1;
pub const WM_EZ_SAVE: u32 = WM_APP + 2;
pub const WM_EZ_QUIT: u32 = WM_APP + 3;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("ezclip");

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn recorder_hwnd() -> Option<HWND> {
    unsafe { FindWindowW(RECORDER_CLASS, PCWSTR::null()).ok().filter(|h| !h.is_invalid()) }
}

pub fn post_to_recorder(msg: u32) -> bool {
    match recorder_hwnd() {
        Some(h) => unsafe { PostMessageW(h, msg, WPARAM(0), LPARAM(0)).is_ok() },
        None => false,
    }
}

pub fn spawn_self(arg: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(arg).spawn();
    }
}

pub fn videos_dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Videos, KNOWN_FOLDER_FLAG(0), HANDLE::default()) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Videos")).unwrap_or_else(|| PathBuf::from("C:\\"))
}

// ---------------------------------------------------------------- startup ---

fn startup_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

pub fn startup_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_READ, &mut key).is_err() {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let r = RegQueryValueExW(key, RUN_VALUE, None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut len));
        let _ = RegCloseKey(key);
        if r.is_err() {
            return false;
        }
        let val = String::from_utf16_lossy(&buf[..(len as usize / 2)]);
        let val = val.trim_end_matches('\0');
        // Only "on" if it points at this exe (the user may have moved it).
        startup_command().map(|c| c.eq_ignore_ascii_case(val)).unwrap_or(false)
    }
}

pub fn set_startup(enable: bool) -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
            .is_err()
        {
            return false;
        }
        let ok = if enable {
            match startup_command() {
                Some(cmd) => {
                    let w = wide(&cmd);
                    let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
                    RegSetValueExW(key, RUN_VALUE, 0, REG_SZ, Some(bytes)).is_ok()
                }
                None => false,
            }
        } else {
            let r = RegDeleteValueW(key, RUN_VALUE);
            r.is_ok() || r == ERROR_FILE_NOT_FOUND
        };
        let _ = RegCloseKey(key);
        ok
    }
}

// ------------------------------------------------------------------ shell ---

pub fn open_path(p: &Path) {
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), &HSTRING::from(p.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn show_in_folder(p: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", p.display())).spawn();
}

/// Move a file to the recycle bin (so a mis-click is recoverable).
pub fn recycle(p: &Path) -> bool {
    let mut from: Vec<u16> = p.as_os_str().encode_wide_vec();
    from.push(0);
    from.push(0); // double-null terminated list
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

trait EncodeWideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}
impl EncodeWideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

/// Native folder picker. Must be called from a thread where COM can be initialised.
pub fn pick_folder(start: &Path) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
        let _ = dlg.SetTitle(w!("Choose where clips are saved"));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(HWND::default()).ok()?;
        let item = dlg.GetResult().ok()?;
        let name: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        s.map(PathBuf::from)
    }
}

/// Bring an already-open ezclip window to the front (used for single-instance UI).
pub fn focus_existing_ui() {
    unsafe {
        if let Ok(h) = FindWindowW(PCWSTR::null(), w!("ezclip")) {
            if !h.is_invalid() {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
            }
        }
    }
}

/// Named mutex used for single-instance checks. Returns None if another
/// instance already holds it. The handle must be kept alive.
pub fn single_instance(name: &str) -> Option<HANDLE> {
    unsafe {
        let h = windows::Win32::System::Threading::CreateMutexW(None, true, &HSTRING::from(name)).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            None
        } else {
            Some(h)
        }
    }
}
