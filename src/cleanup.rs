//! Auto-cleanup: keeps the clips folder under a size and/or age limit by
//! removing the oldest clips. Only clips made by ezclip are considered, and
//! favourites are never removed.
use crate::clipmeta;
use crate::config::Config;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub struct Plan {
    /// Oldest first.
    pub remove: Vec<(PathBuf, u64)>,
    pub ezclip_bytes: u64,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.remove.iter().map(|(_, b)| b).sum()
    }
}

struct Entry {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    favorite: bool,
}

fn ezclip_clips(cfg: &Config) -> Vec<Entry> {
    let mut dirs = vec![cfg.clips_dir.clone()];
    if let Ok(rd) = std::fs::read_dir(&cfg.clips_dir) {
        dirs.extend(rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()));
    }
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() || !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")) || !clipmeta::is_ezclip(&path) {
                continue;
            }
            out.push(Entry {
                favorite: clipmeta::is_favorite(&path),
                path,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    out.sort_by_key(|e| e.modified);
    out
}

/// What cleanup would remove right now (used for the Settings preview too).
pub fn plan(cfg: &Config) -> Plan {
    let clips = ezclip_clips(cfg);
    let total: u64 = clips.iter().map(|e| e.size).sum();
    let mut remove = vec![false; clips.len()];
    if cfg.cleanup_max_days > 0 {
        let cutoff = SystemTime::now() - Duration::from_secs(cfg.cleanup_max_days as u64 * 86_400);
        for (i, e) in clips.iter().enumerate() {
            if !e.favorite && e.modified < cutoff {
                remove[i] = true;
            }
        }
    }
    if cfg.cleanup_max_gb > 0 {
        let limit = cfg.cleanup_max_gb as u64 * 1_000_000_000;
        let mut left: u64 = clips.iter().zip(&remove).filter(|(_, r)| !**r).map(|(e, _)| e.size).sum();
        for (i, e) in clips.iter().enumerate() {
            if left <= limit {
                break;
            }
            if !e.favorite && !remove[i] {
                remove[i] = true;
                left -= e.size;
            }
        }
    }
    Plan {
        remove: clips.into_iter().zip(remove).filter(|(_, r)| *r).map(|(e, _)| (e.path, e.size)).collect(),
        ezclip_bytes: total,
    }
}

/// Apply the plan (if cleanup is on). Returns how many clips were removed.
pub fn run(cfg: &Config) -> usize {
    if !cfg.cleanup_enabled || (cfg.cleanup_max_gb == 0 && cfg.cleanup_max_days == 0) {
        return 0;
    }
    let mut n = 0;
    for (p, size) in plan(cfg).remove {
        let ok = if cfg.cleanup_recycle { crate::win::recycle(&p) } else { std::fs::remove_file(&p).is_ok() };
        crate::log::write(&format!("cleanup: {} {} ({} MB)", if ok { "removed" } else { "could not remove" }, p.display(), size / 1_000_000));
        n += ok as usize;
    }
    n
}
