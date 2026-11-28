//! In-app updates: ask GitHub for the latest release, download ezclip.exe,
//! verify it, swap it in place of the running exe, and restart.
//!
//! Windows lets a running exe be *renamed* (not overwritten), so the swap is:
//! ezclip.exe → ezclip.exe.old, new file → ezclip.exe. If the folder needs
//! admin rights (e.g. Program Files), the downloaded exe is started elevated
//! with `--install-update <target>` to do the same swap.
use crate::release::{self, Release};
use std::path::{Path, PathBuf};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

pub type Result<T> = std::result::Result<T, String>;

/// Closes a WinHTTP handle when dropped.
struct H(*mut core::ffi::c_void);
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// HTTPS GET through Windows' own HTTP stack (system proxy and certificates).
/// Redirects (GitHub → its download CDN) are followed automatically.
fn get(url: &str, accept: &str, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>> {
    let rest = url.strip_prefix("https://").ok_or("only https downloads are allowed")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    unsafe {
        let agent = HSTRING::from(format!("ezclip/{}", env!("CARGO_PKG_VERSION")));
        let session = H(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(format!("can't start a connection ({})", windows::core::Error::from_win32()));
        }
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 60_000);
        let conn = H(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT as u16, 0));
        if conn.0.is_null() {
            return Err(format!("can't reach {host} ({})", windows::core::Error::from_win32()));
        }
        let req = H(WinHttpOpenRequest(conn.0, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if req.0.is_null() {
            return Err(format!("request failed ({})", windows::core::Error::from_win32()));
        }
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(req.0, Some(&headers), None, 0, 0, 0).map_err(|e| format!("no internet connection? ({e})"))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("no answer from GitHub ({e})"))?;

        let number = |what: u32| -> Option<u32> {
            let mut v = 0u32;
            let mut len = 4u32;
            let mut index = 0u32;
            WinHttpQueryHeaders(req.0, what | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut v as *mut u32 as _), &mut len, &mut index)
                .ok()
                .map(|_| v)
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        if status != 200 {
            return Err(match status {
                403 | 429 => "GitHub is rate-limiting update checks. Try again in a bit".into(),
                404 => "no release found on GitHub".into(),
                s => format!("GitHub answered with HTTP {s}"),
            });
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH).map(|v| v as u64);
        let mut body = Vec::with_capacity(total.unwrap_or(64 * 1024) as usize);
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(|e| format!("download interrupted ({e})"))?;
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, body[start..].as_mut_ptr() as _, avail, &mut read).map_err(|e| format!("download interrupted ({e})"))?;
            body.truncate(start + read as usize);
            progress(body.len() as u64, total);
        }
        Ok(body)
    }
}

/// Ask GitHub for the latest release.
pub fn check() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", release::REPO);
    let body = get(&url, "application/vnd.github+json", |_, _| {})?;
    release::parse_latest(&String::from_utf8_lossy(&body))
}

fn update_dir() -> PathBuf {
    std::env::temp_dir().join("ezclip-update")
}

/// Download and verify the new exe. Returns where it was saved.
pub fn download(rel: &Release, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    let data = get(&rel.url, "application/octet-stream", |got, total| {
        let total = total.unwrap_or(rel.size).max(1);
        progress((got as f64 / total as f64).min(1.0) as f32);
    })?;
    if rel.size > 0 && data.len() as u64 != rel.size {
        return Err("the download was incomplete. Please try again".into());
    }
    if let Some(expected) = &rel.sha256 {
        let got = crate::sha256::hex(&crate::sha256::sha256(&data));
        if &got != expected {
            return Err("the download didn't match GitHub's checksum, so it wasn't installed".into());
        }
    }
    if !data.starts_with(b"MZ") {
        return Err("the download isn't a Windows program".into());
    }
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("ezclip-{}.exe", rel.version));
    std::fs::write(&path, &data).map_err(|e| format!("can't save the download: {e}"))?;
    Ok(path)
}

/// Replace `target` with `new_exe` (works while `target` is running).
fn swap(new_exe: &Path, target: &Path) -> std::io::Result<()> {
    let old = target.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old)?;
    if let Err(e) = std::fs::copy(new_exe, target) {
        let _ = std::fs::rename(&old, target);
        return Err(e);
    }
    Ok(())
}

/// Put the downloaded exe in place of the running one. Asks for admin rights
/// only if the folder needs them.
pub fn install(new_exe: &Path) -> Result<PathBuf> {
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    match swap(new_exe, &target) {
        Ok(()) => Ok(target),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            crate::log::write(&format!("update: {} needs admin rights, asking", target.display()));
            run_elevated(new_exe, &target)?;
            Ok(target)
        }
        Err(e) => Err(format!("couldn't replace ezclip.exe: {e}")),
    }
}

/// Run `new_exe --install-update "<target>"` as administrator and wait for it.
fn run_elevated(new_exe: &Path, target: &Path) -> Result<()> {
    unsafe {
        let file = HSTRING::from(new_exe.as_os_str());
        let params = HSTRING::from(format!("--install-update \"{}\"", target.display()));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|_| "the update needs administrator rights (cancelled)".to_string())?;
        if info.hProcess.is_invalid() {
            return Err("couldn't start the installer".into());
        }
        WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code != 0 {
            return Err("installing the update failed (see the log)".into());
        }
    }
    Ok(())
}

/// Entry point for `ezclip.exe --install-update <target>` (run elevated).
pub fn install_entry(target: &str) -> i32 {
    let Ok(me) = std::env::current_exe() else { return 1 };
    match swap(&me, Path::new(target)) {
        Ok(()) => 0,
        Err(e) => {
            crate::log::write(&format!("update: elevated install failed: {e}"));
            1
        }
    }
}

/// Stop the old recorder, start the new version (recorder + window), and exit.
pub fn restart(target: &Path) -> ! {
    crate::win::post_to_recorder(crate::win::WM_EZ_QUIT);
    for _ in 0..50 {
        if crate::win::recorder_hwnd().is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let pid = std::process::id().to_string();
    let _ = std::process::Command::new(target).arg("--background").spawn();
    // The new window waits for this one to close (single-instance lock).
    let _ = std::process::Command::new(target).args(["--ui", "--wait-pid", &pid]).spawn();
    std::process::exit(0);
}

/// Wait (up to 10 s) for process `pid` to exit. Used by the restarted window.
pub fn wait_for_pid(pid: u32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
}

/// After an update: remove the previous exe and the downloaded copies.
pub fn cleanup() {
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::remove_file(me.with_extension("exe.old"));
    }
    let _ = std::fs::remove_dir_all(update_dir());
}
