//! Tiny file logger (%APPDATA%\ezclip\<role>.log). There is no console, so
//! this is the only place errors end up besides the status shown in the UI.
use std::io::Write;
use std::sync::Mutex;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init(role: &str) {
    let dir = crate::config::app_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{role}.log"));
    // Keep the log small: start fresh once it grows past 512 KB.
    let truncate = std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(!truncate)
        .write(true)
        .truncate(truncate)
        .open(path)
        .ok();
    *FILE.lock().unwrap() = f;
    write("---- started ----");
}

pub fn write(msg: &str) {
    let line = format!("{} {msg}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"));
    if let Some(f) = FILE.lock().unwrap().as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
    #[cfg(debug_assertions)]
    eprint!("{line}");
}
