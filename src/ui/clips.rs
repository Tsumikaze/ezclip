//! Clip library: folder scan + background thumbnail/duration loading.
//! Thumbnails come from the Windows shell (the same ones Explorer shows), so
//! no video decoding happens in-process.
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;
use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

pub struct Clip {
    pub path: PathBuf,
    pub name: String,
    /// Game folder the clip lives in (None = directly in the clips folder).
    pub game: Option<String>,
    pub favorite: bool,
    pub modified: SystemTime,
    pub size: u64,
    pub duration: Option<f64>,
    pub thumb: Option<egui::TextureHandle>,
    requested: bool,
}

pub struct Loaded {
    path: PathBuf,
    image: Option<egui::ColorImage>,
    duration: Option<f64>,
}

pub struct Library {
    pub clips: Vec<Clip>,
    tx: Sender<PathBuf>,
    rx: Receiver<Loaded>,
}

impl Library {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, worker_rx) = channel::<PathBuf>();
        let (worker_tx, rx) = channel::<Loaded>();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                while let Ok(path) = worker_rx.recv() {
                    let image = shell_thumbnail(&path);
                    let duration = mp4_duration(&path);
                    if worker_tx.send(Loaded { path, image, duration }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .unwrap();
        Self { clips: Vec::new(), tx, rx }
    }

    /// Re-read the clips folder and its game subfolders (one level deep),
    /// keeping already loaded thumbnails.
    pub fn scan(&mut self, dir: &Path) {
        let mut old: HashMap<PathBuf, Clip> = self.clips.drain(..).map(|c| (c.path.clone(), c)).collect();
        let mut dirs: Vec<(PathBuf, Option<String>)> = vec![(dir.to_path_buf(), None)];
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    dirs.push((e.path(), Some(e.file_name().to_string_lossy().to_string())));
                }
            }
        }
        for (d, game) in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                let is_video = path
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| ["mp4", "mkv", "mov"].iter().any(|v| x.eq_ignore_ascii_case(v)));
                let Ok(meta) = e.metadata() else { continue };
                if !is_video || !meta.is_file() {
                    continue;
                }
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let size = meta.len();
                match old.remove(&path) {
                    Some(c) if c.size == size && c.modified == modified => self.clips.push(c),
                    _ => self.clips.push(Clip {
                        name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        game: game.clone(),
                        favorite: crate::clipmeta::is_favorite(&path),
                        path,
                        modified,
                        size,
                        duration: None,
                        thumb: None,
                        requested: false,
                    }),
                }
            }
        }
        self.clips.sort_by(|a, b| b.modified.cmp(&a.modified));
    }

    /// Game folder names that have clips, sorted.
    pub fn games(&self) -> Vec<String> {
        let mut g: Vec<String> = self.clips.iter().filter_map(|c| c.game.clone()).collect();
        g.sort_by_key(|s| s.to_lowercase());
        g.dedup();
        g
    }

    /// Ask the worker for thumbnails we don't have yet; receive finished ones.
    pub fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(l) = self.rx.try_recv() {
            if let Some(c) = self.clips.iter_mut().find(|c| c.path == l.path) {
                c.duration = l.duration;
                c.thumb = l.image.map(|img| ctx.load_texture(c.path.to_string_lossy(), img, egui::TextureOptions::LINEAR));
            }
        }
        for c in self.clips.iter_mut().filter(|c| !c.requested) {
            c.requested = true;
            let _ = self.tx.send(c.path.clone());
        }
    }

    pub fn total_size(&self) -> u64 {
        self.clips.iter().map(|c| c.size).sum()
    }
}

fn mp4_duration(path: &Path) -> Option<f64> {
    crate::clipfile::quick_info(path).map(|q| q.duration).filter(|d| *d > 0.0)
}

fn shell_thumbnail(path: &Path) -> Option<egui::ColorImage> {
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let hbmp = factory.GetImage(SIZE { cx: 320, cy: 180 }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let mut bm = BITMAP::default();
        GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _));
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            let _ = DeleteObject(hbmp);
            return None;
        }
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let hdc = GetDC(None);
        let lines = GetDIBits(hdc, hbmp, 0, h as u32, Some(px.as_mut_ptr() as _), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, hdc);
        let _ = DeleteObject(hbmp);
        if lines == 0 {
            return None;
        }
        let all_transparent = px.chunks(4).all(|p| p[3] == 0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2); // BGRA -> RGBA
            if all_transparent {
                p[3] = 255;
            }
        }
        Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px))
    }
}
