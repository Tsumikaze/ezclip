//! Settings, persisted as JSON in %APPDATA%\ezclip\config.json.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    Native,
    P1440,
    P1080,
    P720,
}

impl Resolution {
    pub const ALL: [Resolution; 4] = [Self::Native, Self::P1440, Self::P1080, Self::P720];
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::P1440 => "1440p",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
        }
    }
    fn max_height(self) -> Option<u32> {
        match self {
            Self::Native => None,
            Self::P1440 => Some(1440),
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
        }
    }
    /// Output size for a given desktop size: keeps aspect ratio, never upscales,
    /// always even (required for NV12 / H.264).
    pub fn output_size(self, w: u32, h: u32) -> (u32, u32) {
        let (mut ow, mut oh) = (w, h);
        if let Some(maxh) = self.max_height() {
            if h > maxh {
                oh = maxh;
                ow = ((w as u64 * maxh as u64 + h as u64 / 2) / h as u64) as u32;
            }
        }
        (ow.max(2) & !1, oh.max(2) & !1)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    Low,
    Medium,
    High,
    Ultra,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }
    /// Target video bitrate (bits/s) for an output size and frame rate.
    pub fn bitrate(self, w: u32, h: u32, fps: u32) -> u32 {
        let bits_per_pixel = match self {
            Self::Low => 0.07,
            Self::Medium => 0.12,
            Self::High => 0.20,
            Self::Ultra => 0.32,
        };
        // Frame rate scales sub-linearly: higher fps frames are more similar.
        let eff_fps = 60.0 * (fps as f64 / 60.0).powf(0.6);
        let b = w as f64 * h as f64 * eff_fps * bits_per_pixel;
        b.clamp(2_000_000.0, 100_000_000.0) as u32
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Self::H264, Self::Hevc, Self::Av1];
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
        }
    }
    /// Newer codecs reach the same quality with fewer bits.
    pub fn bitrate_factor(self) -> f64 {
        match self {
            Self::H264 => 1.0,
            Self::Hevc => 0.7,
            Self::Av1 => 0.6,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows virtual-key code.
    pub vk: u32,
}

impl Default for Hotkey {
    fn default() -> Self {
        // Alt + F10
        Self { ctrl: false, alt: true, shift: false, vk: 0x79 }
    }
}

impl Hotkey {
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl + ";
        }
        if self.alt {
            s += "Alt + ";
        }
        if self.shift {
            s += "Shift + ";
        }
        s += &vk_name(self.vk);
        s
    }
}

pub fn vk_name(vk: u32) -> String {
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap().to_string(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x20 => "Space".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x13 => "Pause".into(),
        0x91 => "Scroll Lock".into(),
        0x2C => "Print Screen".into(),
        0xC0 => "`".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xDC => "\\".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        _ => format!("Key {vk}"),
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub clip_seconds: u32,
    pub fps: u32,
    pub resolution: Resolution,
    pub quality: Quality,
    pub codec: Codec,
    /// Index into the list of desktop monitors (see `display::monitors`).
    pub monitor: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Recording device id; None = Windows' default microphone.
    pub mic_device: Option<String>,
    /// Microphone volume in percent (100 = as recorded).
    pub mic_volume: u32,
    pub hotkey: Hotkey,
    pub clips_dir: PathBuf,
    /// Save into a subfolder matching the game (e.g. E:\videos\Valorant).
    pub game_folders: bool,
    /// Create a folder for games that don't have one yet.
    pub new_game_folders: bool,
    /// Put each new clip on the clipboard (ready to paste into Discord).
    pub copy_after_save: bool,
    pub sound_on_save: bool,
    pub save_sound: crate::sound::SaveSound,
    /// 0–100.
    pub save_volume: u32,
    pub notify_on_save: bool,
    /// Auto-cleanup of old ezclip clips (favourites are kept).
    pub cleanup_enabled: bool,
    /// 0 = no size limit.
    pub cleanup_max_gb: u32,
    /// 0 = no age limit.
    pub cleanup_max_days: u32,
    /// Move to the Recycle Bin instead of deleting permanently.
    pub cleanup_recycle: bool,
    /// Target size for "Shrink for Discord".
    pub discord_limit_mb: u32,
    pub copy_after_shrink: bool,
    /// The Copy button shrinks clips that are over the Discord limit first.
    pub copy_fits_discord: bool,
    /// Look for a newer version on GitHub when the window opens.
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            clip_seconds: 30,
            fps: 60,
            resolution: Resolution::Native,
            quality: Quality::High,
            codec: Codec::H264,
            monitor: 0,
            system_audio: true,
            microphone: false,
            mic_device: None,
            mic_volume: 100,
            hotkey: Hotkey::default(),
            clips_dir: crate::win::videos_dir().join("ezclip"),
            game_folders: true,
            new_game_folders: true,
            copy_after_save: false,
            sound_on_save: true,
            save_sound: crate::sound::SaveSound::Chirp,
            save_volume: 50,
            notify_on_save: false,
            cleanup_enabled: false,
            cleanup_max_gb: 100,
            cleanup_max_days: 0,
            cleanup_recycle: true,
            discord_limit_mb: 10,
            copy_after_shrink: true,
            copy_fits_discord: true,
            check_updates: true,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
            .map(|mut c| {
                c.clip_seconds = c.clip_seconds.clamp(5, 600);
                c.fps = c.fps.clamp(15, 240);
                c
            })
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(app_dir())?;
        let tmp = config_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap())?;
        std::fs::rename(tmp, config_path())
    }

    /// Settings whose change requires restarting the capture pipeline.
    /// (Mic volume is applied live and isn't part of it.)
    #[allow(clippy::type_complexity)]
    pub fn capture_key(&self) -> (u32, Resolution, Quality, Codec, u32, bool, bool, Option<String>) {
        (self.fps, self.resolution, self.quality, self.codec, self.monitor, self.system_audio, self.microphone, self.mic_device.clone())
    }

    /// Video bitrate for an output size with the chosen quality and codec.
    pub fn video_bitrate(&self, w: u32, h: u32, codec: Codec) -> u32 {
        (self.quality.bitrate(w, h, self.fps) as f64 * codec.bitrate_factor()) as u32
    }
}

pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ezclip")
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn status_path() -> PathBuf {
    app_dir().join("status.json")
}

/// What the recorder reports to the UI.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Status {
    pub ok: bool,
    pub video: String,
    pub audio: String,
    pub error: String,
}
