//! ezclip — lightweight instant-replay clipper for Windows.
//!
//! One executable, two roles:
//!   `ezclip.exe --background`  tray recorder (keeps the last N seconds in RAM)
//!   `ezclip.exe`               opens the window (and starts the recorder if needed)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("ezclip only supports Windows");

mod cleanup;
mod clipfile;
mod clipmeta;
mod config;
mod display;
mod game_match;
mod icon;
mod log;
mod recorder;
mod ui;
mod win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a.eq_ignore_ascii_case(f));

    // First run: save defaults and start with Windows (can be turned off in Settings).
    if !config::config_path().exists() {
        let _ = config::Config::default().save();
        win::set_startup(true);
    }

    if has("--background") {
        log::init("recorder");
        if let Err(e) = recorder::run() {
            log::write(&format!("recorder exited with error: {e:?}"));
        }
        return;
    }

    log::init("ui");
    // Opening the window always makes sure the recorder is running too.
    if !has("--ui") && win::recorder_hwnd().is_none() {
        win::spawn_self("--background");
    }
    if let Err(e) = ui::run() {
        log::write(&format!("ui exited with error: {e:?}"));
    }
}
