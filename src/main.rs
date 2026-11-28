//! ezclip — lightweight instant-replay clipper for Windows.
//!
//! One executable, two roles:
//!   `ezclip.exe --background`  tray recorder (keeps the last N seconds in RAM)
//!   `ezclip.exe`               opens the window (and starts the recorder if needed)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("ezclip only supports Windows");

mod audiodev;
mod cleanup;
mod clipfile;
mod clipmeta;
mod config;
mod display;
mod game_match;
mod icon;
mod log;
mod mp4w;
mod recorder;
mod release;
mod sha256;
mod sound;
mod ui;
mod update;
mod win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a.eq_ignore_ascii_case(f));
    let value = |f: &str| args.iter().position(|a| a.eq_ignore_ascii_case(f)).and_then(|i| args.get(i + 1)).cloned();

    // Updater, elevated: copy this (new) exe over the installed one, then exit.
    if let Some(target) = value("--install-update") {
        log::init("update");
        std::process::exit(update::install_entry(&target));
    }
    // Restarted after an update: let the old window close first.
    if let Some(pid) = value("--wait-pid").and_then(|p| p.parse().ok()) {
        update::wait_for_pid(pid);
    }
    update::cleanup();

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
