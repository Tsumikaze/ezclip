//! Per-clip flags stored with the file itself:
//! * "favourite": never removed by auto-cleanup.
//! * "made by ezclip": auto-cleanup only ever considers these.
//!
//! Both are tiny NTFS alternate data streams (`clip.mp4:ezclip.favorite`),
//! so they survive renames and moves in Explorer. On drives without stream
//! support (FAT/exFAT) favourites fall back to a list in %APPDATA%.
use std::path::{Path, PathBuf};

const FAV: &str = "ezclip.favorite";
const MARK: &str = "ezclip.clip";

fn stream(p: &Path, name: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(":");
    s.push(name);
    PathBuf::from(s)
}

fn has_stream(p: &Path, name: &str) -> bool {
    std::fs::File::open(stream(p, name)).is_ok()
}

fn fav_list_path() -> PathBuf {
    crate::config::app_dir().join("favorites.json")
}

fn fav_list() -> Vec<String> {
    std::fs::read(fav_list_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_fav_list(list: &[String]) {
    let _ = std::fs::create_dir_all(crate::config::app_dir());
    let _ = std::fs::write(fav_list_path(), serde_json::to_vec_pretty(list).unwrap_or_default());
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

pub fn is_favorite(p: &Path) -> bool {
    has_stream(p, FAV) || fav_list().contains(&key(p))
}

pub fn set_favorite(p: &Path, on: bool) -> bool {
    let mut list = fav_list();
    list.retain(|k| *k != key(p));
    if on {
        if std::fs::write(stream(p, FAV), b"1").is_err() {
            list.push(key(p));
        }
    } else {
        let _ = std::fs::remove_file(stream(p, FAV));
    }
    save_fav_list(&list);
    is_favorite(p) == on
}

/// Keep the fallback favourites list in step with a rename done by ezclip.
pub fn renamed(from: &Path, to: &Path) {
    let mut list = fav_list();
    if let Some(e) = list.iter_mut().find(|k| **k == key(from)) {
        *e = key(to);
        save_fav_list(&list);
    }
}

pub fn mark_ezclip(p: &Path) {
    let _ = std::fs::write(stream(p, MARK), b"1");
}

/// Made by ezclip: has the marker, or (drives without streams) still has
/// ezclip's "<name> YYYY-MM-DD HH-MM-SS.mp4" file name.
pub fn is_ezclip(p: &Path) -> bool {
    has_stream(p, MARK) || has_ezclip_name(p)
}

pub fn has_ezclip_name(p: &Path) -> bool {
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        return false;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // strip " (2)" style suffix
    let stem = match stem.rfind(" (") {
        Some(i) if stem.ends_with(')') && stem[i + 2..stem.len() - 1].chars().all(|c| c.is_ascii_digit()) => stem[..i].to_string(),
        _ => stem,
    };
    let b = stem.as_bytes();
    if b.len() < 20 {
        return false;
    }
    let t = &b[b.len() - 19..]; // "YYYY-MM-DD HH-MM-SS"
    let pattern = b"dddd-dd-dd dd-dd-dd";
    b[b.len() - 20] == b' '
        && t.iter().zip(pattern.iter()).all(|(c, p)| if *p == b'd' { c.is_ascii_digit() } else { c == p })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognises_ezclip_names() {
        assert!(has_ezclip_name(Path::new("E:\\videos\\Valorant\\Valorant 2026-11-25 21-14-03.mp4")));
        assert!(has_ezclip_name(Path::new("Desktop 2026-11-25 21-14-03 (2).mp4")));
        assert!(!has_ezclip_name(Path::new("1v4 clutch.mp4")));
        assert!(!has_ezclip_name(Path::new("Valorant 2024.01.01 - 12.00.00.01.mp4")));
        assert!(!has_ezclip_name(Path::new("Valorant 2026-11-25 21-14-03.mkv")));
    }
}
