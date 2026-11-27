//! The ezclip window: a Clips library and a Settings page.
//! Runs as its own short-lived process, so it costs nothing when closed.
mod clips;
mod editor;
mod media;
mod theme;
mod widgets;

use crate::config::{vk_name, Codec, Config, Hotkey, Quality, Resolution, Status};
use crate::win;
use clips::Library;
use editor::{Editor, EditorAction};
use media::{FrameGrabber, HoverGrabber, ShrinkUpdate, HOVER_FRAMES};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use theme::*;
use widgets::*;

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Clips,
    Settings,
    Editor,
}

#[derive(PartialEq, Clone, Copy)]
enum Sort {
    Newest,
    Oldest,
    Largest,
    Longest,
    Name,
}

impl Sort {
    const ALL: [Sort; 5] = [Sort::Newest, Sort::Oldest, Sort::Largest, Sort::Longest, Sort::Name];
    fn label(self) -> &'static str {
        match self {
            Sort::Newest => "Newest first",
            Sort::Oldest => "Oldest first",
            Sort::Largest => "Largest first",
            Sort::Longest => "Longest first",
            Sort::Name => "Name (A–Z)",
        }
    }
}

type TrimResult = Result<(PathBuf, bool), String>;

struct App {
    cfg: Config,
    page: Page,
    lib: Library,
    last_scan: Instant,
    monitors: Vec<(String, u32, u32)>,
    recorder_running: bool,
    status: Option<Status>,
    last_status: Instant,
    startup: bool,
    capturing_hotkey: bool,
    hotkey_hint: Option<String>,
    renaming: Option<(PathBuf, String)>,
    folder_edit: String,
    toast: Option<(String, Instant)>,
    logo: egui::TextureHandle,
    ctx: egui::Context,
    // clips list filters
    search: String,
    game_filter: Option<String>,
    sort: Sort,
    favorites_only: bool,
    // trim editor
    editor: Option<Editor>,
    grabber: Option<FrameGrabber>,
    trim_tx: Sender<TrimResult>,
    trim_rx: Receiver<TrimResult>,
    // shrink for Discord: path -> progress
    shrinking: HashMap<PathBuf, f32>,
    /// Shrinks started by the Copy button: copy the result when done.
    copy_when_shrunk: std::collections::HashSet<PathBuf>,
    /// Target size of each running shrink (for its messages).
    shrink_limits: HashMap<PathBuf, u32>,
    shrink_tx: Sender<ShrinkUpdate>,
    shrink_rx: Receiver<ShrinkUpdate>,
    cleanup_preview: Option<(String, bool)>,
    // hover scrubbing
    hover: Option<HoverGrabber>,
    hover_frames: HashMap<PathBuf, Vec<Option<egui::TextureHandle>>>,
    /// Clips whose frames can't be decoded (don't keep asking).
    hover_failed: std::collections::HashSet<PathBuf>,
    hover_requested: Option<PathBuf>,
}

pub fn run() -> Result<(), String> {
    let Some(_mutex) = win::single_instance("Local\\EzClipUiMutex") else {
        win::focus_existing_ui();
        return Ok(());
    };
    let icon = egui::IconData { rgba: crate::icon::rgba(64), width: 64, height: 64 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ezclip")
            .with_inner_size([1000.0, 660.0])
            .with_min_inner_size([780.0, 500.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("ezclip", opts, Box::new(|cc| Ok(Box::new(App::new(cc))))).map_err(|e| e.to_string())
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        theme::apply(&cc.egui_ctx);
        let cfg = Config::load();
        let mut lib = Library::new(cc.egui_ctx.clone());
        lib.scan(&cfg.clips_dir);
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64)),
            egui::TextureOptions::LINEAR,
        );
        let monitors = crate::display::monitors()
            .iter()
            .enumerate()
            .map(|(i, m)| (crate::display::label(i, m), m.width, m.height))
            .collect();
        let folder_edit = cfg.clips_dir.display().to_string();
        let (trim_tx, trim_rx) = channel();
        let (shrink_tx, shrink_rx) = channel();
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            search: String::new(),
            game_filter: None,
            sort: Sort::Newest,
            favorites_only: false,
            editor: None,
            grabber: None,
            trim_tx,
            trim_rx,
            shrinking: HashMap::new(),
            copy_when_shrunk: Default::default(),
            shrink_limits: HashMap::new(),
            shrink_tx,
            shrink_rx,
            cleanup_preview: None,
            hover: None,
            hover_frames: HashMap::new(),
            hover_failed: Default::default(),
            hover_requested: None,
            cfg,
            page: Page::Clips,
            lib,
            last_scan: Instant::now(),
            monitors,
            recorder_running: false,
            status: None,
            last_status: Instant::now() - Duration::from_secs(10),
            startup: win::startup_enabled(),
            capturing_hotkey: false,
            hotkey_hint: None,
            renaming: None,
            folder_edit,
            toast: None,
            logo,
        };
        app.poll_status();
        app
    }

    fn apply(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.toast(format!("Couldn't save settings: {e}"));
        }
        win::post_to_recorder(win::WM_EZ_RELOAD);
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn poll_status(&mut self) {
        self.last_status = Instant::now();
        self.recorder_running = win::recorder_hwnd().is_some();
        self.status = if self.recorder_running {
            std::fs::read(crate::config::status_path()).ok().and_then(|b| serde_json::from_slice(&b).ok())
        } else {
            None
        };
    }

    // ------------------------------------------------------------ sidebar --

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        // Status + save button pinned to the bottom, navigation on top.
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::none())
            .show_separator_line(false)
            .show_inside(ui, |ui| self.status_card(ui));

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(26.0)));
            ui.add_space(2.0);
            ui.label(RichText::new("ezclip").size(20.0).family(bold()).color(TEXT));
            ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).size(11.5).color(TEXT_FAINT));
        });
        ui.add_space(26.0);

        let count = self.lib.clips.len();
        for (page, label, badge) in [
            (Page::Clips, "Clips", if count > 0 { count.to_string() } else { String::new() }),
            (Page::Settings, "Settings", String::new()),
        ] {
            let active = self.page == page || (page == Page::Clips && self.page == Page::Editor);
            if self.nav_item(ui, label, &badge, active).clicked() {
                self.page = page;
                self.capturing_hotkey = false;
                self.editor = None;
                self.cleanup_preview = None;
            }
        }
    }

    fn nav_item(&self, ui: &mut egui::Ui, label: &str, badge: &str, active: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
        let hovered = resp.hovered();
        let p = ui.painter();
        if active {
            p.rect_filled(rect, R, Color32::from_rgb(26, 26, 26));
            p.rect_filled(egui::Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 10.0), Vec2::new(3.0, 18.0)), 2.0, ACCENT);
        } else if hovered {
            p.rect_filled(rect, R, Color32::from_rgb(14, 14, 14));
        }
        let color = if active { TEXT } else if hovered { Color32::from_rgb(200, 200, 200) } else { TEXT_DIM };
        p.text(
            rect.left_center() + Vec2::new(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(14.5, if active { bold() } else { egui::FontFamily::Proportional }),
            color,
        );
        if !badge.is_empty() {
            p.text(rect.right_center() - Vec2::new(12.0, 0.0), egui::Align2::RIGHT_CENTER, badge, egui::FontId::proportional(12.0), TEXT_FAINT);
        }
        ui.add_space(2.0);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn status_card(&mut self, ui: &mut egui::Ui) {
        let (dot, title, lines): (Color32, &str, Vec<String>) = match (&self.status, self.recorder_running) {
            (_, false) => (TEXT_FAINT, "Not running", vec!["Nothing is being recorded".into()]),
            (None, true) => (AMBER, "Starting…", vec![]),
            (Some(s), true) if !s.error.is_empty() && !s.ok => (RED, "Problem", vec![s.error.clone()]),
            (Some(s), true) => {
                let mut l = vec![s.video.clone()];
                if !s.audio.is_empty() {
                    l.push(s.audio.clone());
                }
                if !s.error.is_empty() {
                    l.push(s.error.clone());
                }
                (if s.error.is_empty() { GREEN } else { AMBER }, "Replay buffer on", l)
            }
        };
        egui::Frame::none()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, BORDER))
            .rounding(R)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.5, dot);
                    ui.label(RichText::new(title).size(13.5).family(bold()));
                });
                for l in lines.iter().filter(|l| !l.is_empty()) {
                    ui.add(egui::Label::new(RichText::new(l).size(12.0).color(TEXT_DIM)).wrap());
                }
            });
        ui.add_space(10.0);

        let w = ui.available_width();
        if self.recorder_running {
            if primary_button(ui, &format!("Save last {}", fmt_secs(self.cfg.clip_seconds)), w).clicked() {
                win::post_to_recorder(win::WM_EZ_SAVE);
                self.toast("Saving clip…");
                self.last_scan = Instant::now() - Duration::from_secs(1);
            }
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("or press {}", self.cfg.hotkey.label())).size(12.0).color(TEXT_FAINT));
            });
        } else if primary_button(ui, "Start recording", w).clicked() {
            win::spawn_self("--background");
            self.last_status = Instant::now() - Duration::from_millis(700);
        }
        ui.add_space(2.0);
    }

    // -------------------------------------------------------------- clips --

    /// Indices of clips passing the search / game / favourites filters, sorted.
    fn visible_clips(&self) -> Vec<usize> {
        let words: Vec<String> = self.search.split_whitespace().map(|w| w.to_lowercase()).collect();
        let mut v: Vec<usize> = (0..self.lib.clips.len())
            .filter(|&i| {
                let c = &self.lib.clips[i];
                if self.favorites_only && !c.favorite {
                    return false;
                }
                if let Some(g) = &self.game_filter {
                    if c.game.as_deref() != Some(g.as_str()) {
                        return false;
                    }
                }
                let hay = format!("{} {}", c.name, c.game.as_deref().unwrap_or("")).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        let clips = &self.lib.clips;
        match self.sort {
            Sort::Newest => v.sort_by(|&a, &b| clips[b].modified.cmp(&clips[a].modified)),
            Sort::Oldest => v.sort_by(|&a, &b| clips[a].modified.cmp(&clips[b].modified)),
            Sort::Largest => v.sort_by(|&a, &b| clips[b].size.cmp(&clips[a].size)),
            Sort::Longest => v.sort_by(|&a, &b| clips[b].duration.unwrap_or(0.0).total_cmp(&clips[a].duration.unwrap_or(0.0))),
            Sort::Name => v.sort_by_key(|&i| clips[i].name.to_lowercase()),
        }
        v
    }

    fn clips_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Clips");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Open folder").clicked() {
                    let _ = std::fs::create_dir_all(&self.cfg.clips_dir);
                    win::open_path(&self.cfg.clips_dir);
                }
            });
        });
        let n = self.lib.clips.len();
        let visible = self.visible_clips();
        let shown = if visible.len() == n { format!("{n} clip{}", if n == 1 { "" } else { "s" }) } else { format!("{} of {n} clips", visible.len()) };
        ui.label(
            RichText::new(format!("{shown} · {} · {}", fmt_size(self.lib.total_size()), self.cfg.clips_dir.display()))
                .size(12.5)
                .color(TEXT_FAINT),
        );
        ui.add_space(10.0);

        if n == 0 {
            ui.add_space(ui.available_height() * 0.25);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No clips yet").size(18.0).family(bold()));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "Press {} at any time to save the last {}.",
                        self.cfg.hotkey.label(),
                        fmt_secs(self.cfg.clip_seconds)
                    ))
                    .color(TEXT_DIM),
                );
            });
            return;
        }

        // Toolbar: search, game, sort, favourites.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search clips")
                    .desired_width(200.0)
                    .min_size(Vec2::new(0.0, 30.0))
                    .vertical_align(Align::Center),
            );
            let games = self.lib.games();
            let game_label = self.game_filter.clone().unwrap_or_else(|| "All games".into());
            egui::ComboBox::from_id_salt("game_filter").selected_text(game_label).width(170.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.game_filter, None, "All games");
                for g in games {
                    ui.selectable_value(&mut self.game_filter, Some(g.clone()), g);
                }
            });
            egui::ComboBox::from_id_salt("sort").selected_text(self.sort.label()).width(140.0).show_ui(ui, |ui| {
                for s in Sort::ALL {
                    ui.selectable_value(&mut self.sort, s, s.label());
                }
            });
            if chip_toggle(ui, "Favourites", self.favorites_only).clicked() {
                self.favorites_only = !self.favorites_only;
            }
        });
        ui.add_space(10.0);

        if visible.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No clips match").size(16.0).family(bold()));
                if button(ui, "Clear filters").clicked() {
                    self.search.clear();
                    self.game_filter = None;
                    self.favorites_only = false;
                }
            });
            return;
        }

        let mut action: Option<ClipAction> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 114.0, visible.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for &i in &visible[range] {
                if let Some(a) = self.clip_row(ui, i) {
                    action = Some(a);
                }
            }
        });
        if let Some(a) = action {
            self.do_action(a);
        }
    }

    fn clip_row(&mut self, ui: &mut egui::Ui, i: usize) -> Option<ClipAction> {
        let c = &self.lib.clips[i];
        let path = c.path.clone();
        let mut act = None;
        let row_h = 90.0;
        let thumb_w = row_h * 16.0 / 9.0;
        let (rect, row_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h + 16.0), Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        ui.painter().rect(
            rect,
            R + 2.0,
            if hovered { SURFACE_HOVER } else { SURFACE },
            egui::Stroke::new(1.0_f32, BORDER),
        );
        let inner = rect.shrink(8.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
        let ui = &mut child;

        // Thumbnail. Hovering scrubs through a few frames of the clip.
        let (trect, tresp) = ui.allocate_exact_size(Vec2::new(thumb_w, row_h), Sense::click());
        let hover_pos = tresp.hover_pos();
        let scrub = hover_pos.map(|p| ((p.x - trect.left()) / trect.width()).clamp(0.0, 0.999));
        let hover_tex = scrub.and_then(|f| {
            let frames = self.hover_frames.get(&path)?;
            let want = (f * HOVER_FRAMES as f32) as usize;
            // The wanted frame, or the nearest one already decoded.
            (0..HOVER_FRAMES)
                .flat_map(|d| [want.checked_sub(d), Some(want + d)])
                .flatten()
                .find_map(|i| frames.get(i).cloned().flatten())
        });
        match hover_tex.as_ref().or(c.thumb.as_ref()) {
            Some(t) => {
                egui::Image::new(t)
                    .fit_to_exact_size(trect.size())
                    .rounding(6.0)
                    .maintain_aspect_ratio(false)
                    .paint_at(ui, trect);
            }
            None => {
                ui.painter().rect_filled(trect, 6.0, Color32::from_rgb(22, 22, 22));
            }
        }
        if let Some(d) = c.duration {
            let label = fmt_duration(match scrub {
                Some(f) if hover_tex.is_some() => d * f as f64,
                _ => d,
            });
            let font = egui::FontId::new(11.5, bold());
            let galley = ui.painter().layout_no_wrap(label, font, Color32::WHITE);
            let br = egui::Rect::from_min_size(
                trect.right_bottom() - galley.size() - Vec2::new(12.0, 8.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            ui.painter().rect_filled(br, 4.0, Color32::from_black_alpha(200));
            ui.painter().galley(br.min + Vec2::new(4.0, 2.0), galley, Color32::WHITE);
        }
        if let Some(f) = scrub {
            if hover_tex.is_some() {
                // Scrub position along the bottom edge.
                let y = trect.bottom() - 3.0;
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(egui::pos2(trect.left() + 3.0, y), egui::pos2(trect.right() - 3.0, y + 2.0)),
                    1.0,
                    Color32::from_white_alpha(60),
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(egui::pos2(trect.left() + 3.0, y), egui::pos2(trect.left() + 3.0 + (trect.width() - 6.0) * f, y + 2.0)),
                    1.0,
                    Color32::WHITE,
                );
            } else {
                ui.painter().circle_filled(trect.center(), 18.0, Color32::from_black_alpha(170));
                let c0 = trect.center();
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![c0 + Vec2::new(-5.0, -8.0), c0 + Vec2::new(9.0, 0.0), c0 + Vec2::new(-5.0, 8.0)],
                    Color32::WHITE,
                    egui::Stroke::NONE,
                ));
            }
        }
        let want_hover_frames = scrub.is_some();
        if tresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            act = Some(ClipAction::Play(path.clone()));
        }
        ui.add_space(14.0);

        // Title + meta
        let renaming_this = self.renaming.as_ref().is_some_and(|(p, _)| *p == path);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 250.0).max(120.0));
            ui.add_space(18.0);
            if renaming_this {
                let (_, buf) = self.renaming.as_mut().unwrap();
                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(ui.available_width()).font(egui::FontId::new(15.0, bold())));
                r.request_focus();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    act = Some(ClipAction::CommitRename);
                } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                    act = Some(ClipAction::CancelRename);
                }
            } else {
                ui.add(egui::Label::new(RichText::new(&c.name).size(15.0).family(bold()).color(TEXT)).truncate());
            }
            let meta = match &c.game {
                Some(g) => format!("{g}  ·  {}  ·  {}", fmt_date(c.modified), fmt_size(c.size)),
                None => format!("{}  ·  {}", fmt_date(c.modified), fmt_size(c.size)),
            };
            ui.add(egui::Label::new(RichText::new(meta).size(12.5).color(TEXT_DIM)).truncate());
        });

        let favorite = c.favorite;
        let shrink_progress = self.shrinking.get(&path).copied();
        let limit = self.cfg.discord_limit_mb;
        // Actions (right side): the two common ones visible, the rest in a menu.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(6.0);
            if renaming_this {
                if button(ui, "Save name").clicked() {
                    act = Some(ClipAction::CommitRename);
                }
            } else {
                let more = egui::Button::new(RichText::new("···").size(15.0).family(bold()))
                    .fill(CONTROL)
                    .stroke(egui::Stroke::new(1.0_f32, BORDER))
                    .rounding(6.0)
                    .min_size(Vec2::new(36.0, 30.0));
                let r = ui.add(more).on_hover_cursor(egui::CursorIcon::PointingHand);
                egui::popup::popup_below_widget(
                    ui,
                    ui.make_persistent_id(("more", &path)),
                    &r,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(170.0);
                        if let Some(a) = clip_menu(ui, &path, favorite, limit) {
                            act = Some(a);
                        }
                    },
                );
                if r.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(ui.make_persistent_id(("more", &path))));
                }
            }
            if button(ui, "Copy").on_hover_text(format!("Copy to paste into Discord. Clips over {limit} MB are shrunk to fit first")).clicked() {
                act = Some(ClipAction::Copy(path.clone()));
            }
            if button(ui, "Play").clicked() {
                act = Some(ClipAction::Play(path.clone()));
            }
            if star_button(ui, favorite).clicked() {
                act = Some(ClipAction::Favorite(path.clone(), !favorite));
            }
            if let Some(p) = shrink_progress {
                ui.add(egui::ProgressBar::new(p).desired_width(90.0).desired_height(6.0));
                ui.label(RichText::new("Shrinking").size(12.0).color(TEXT_DIM));
            }
        });

        // Right-click anywhere on the row: same menu.
        row_resp.context_menu(|ui| {
            ui.set_min_width(170.0);
            if let Some(a) = clip_menu(ui, &path, favorite, limit) {
                act = Some(a);
            }
        });

        if want_hover_frames {
            self.request_hover_frames(&path);
        }
        if row_resp.double_clicked() && act.is_none() {
            act = Some(ClipAction::Play(path));
        }
        act
    }

    /// Copy a clip so it can be pasted into Discord. If it is over the upload
    /// limit, shrink it first (or reuse an up-to-date "(N MB)" copy) and
    /// copy the small version when it's ready.
    fn copy_for_discord(&mut self, p: PathBuf) {
        let limit_mb = self.cfg.discord_limit_mb;
        let limit = limit_mb as u64 * 1_000_000;
        let meta = std::fs::metadata(&p);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        if !self.cfg.copy_fits_discord || size <= limit {
            if win::copy_files_to_clipboard(&[&p]) {
                self.toast("Copied — paste it into Discord with Ctrl+V");
            } else {
                self.toast("Couldn't access the clipboard — try again");
            }
            return;
        }
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let small = p.with_file_name(format!("{stem} ({limit_mb} MB).mp4"));
        if let (Ok(ms), Ok(mo)) = (std::fs::metadata(&small), meta) {
            let fresh = match (ms.modified(), mo.modified()) {
                (Ok(a), Ok(b)) => a >= b,
                _ => false,
            };
            if fresh && ms.len() <= limit && win::copy_files_to_clipboard(&[&small]) {
                self.toast(format!("Copied the Discord-sized version ({}) — paste with Ctrl+V", fmt_size(ms.len())));
                return;
            }
        }
        self.copy_when_shrunk.insert(p.clone());
        if !self.shrinking.contains_key(&p) {
            self.shrinking.insert(p.clone(), 0.0);
            self.shrink_limits.insert(p.clone(), limit_mb);
            media::shrink(p, limit_mb, self.shrink_tx.clone(), self.ctx.clone());
        }
        self.toast(format!("{} is over {limit_mb} MB — shrinking it first, it'll be copied when ready", fmt_size(size)));
    }

    fn do_action(&mut self, a: ClipAction) {
        match a {
            ClipAction::Trim(p) => {
                let grabber = self.grabber.get_or_insert_with(|| FrameGrabber::new(self.ctx.clone()));
                self.editor = Some(Editor::open(p, grabber));
                self.page = Page::Editor;
            }
            ClipAction::Shrink(p, mb) => {
                if !self.shrinking.contains_key(&p) {
                    self.shrinking.insert(p.clone(), 0.0);
                    self.shrink_limits.insert(p.clone(), mb);
                    media::shrink(p, mb, self.shrink_tx.clone(), self.ctx.clone());
                    self.toast(format!("Shrinking to under {mb} MB…"));
                }
            }
            ClipAction::Favorite(p, on) => {
                if crate::clipmeta::set_favorite(&p, on) {
                    if let Some(c) = self.lib.clips.iter_mut().find(|c| c.path == p) {
                        c.favorite = on;
                    }
                    self.cleanup_preview = None;
                } else {
                    self.toast("Couldn't change the favourite");
                }
            }
            ClipAction::Play(p) => win::open_path(&p),
            ClipAction::Reveal(p) => win::show_in_folder(&p),
            ClipAction::Copy(p) => self.copy_for_discord(p),
            ClipAction::CopyOriginal(p) => {
                if win::copy_files_to_clipboard(&[&p]) {
                    self.toast("Original copied — paste it with Ctrl+V");
                } else {
                    self.toast("Couldn't access the clipboard — try again");
                }
            }
            ClipAction::Delete(p) => {
                if win::recycle(&p) {
                    self.toast("Moved to Recycle Bin");
                } else {
                    self.toast("Couldn't delete — is the clip open in another app?");
                }
                self.lib.scan(&self.cfg.clips_dir);
            }
            ClipAction::StartRename(p) => {
                let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.renaming = Some((p, stem));
            }
            ClipAction::CancelRename => self.renaming = None,
            ClipAction::CommitRename => {
                if let Some((p, name)) = self.renaming.take() {
                    let name = name.trim().to_string();
                    let bad = name.is_empty() || name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']);
                    if bad {
                        self.toast("Names can't be empty or contain \\ / : * ? \" < > |");
                        self.renaming = Some((p, name));
                        return;
                    }
                    let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "mp4".into());
                    let target = p.with_file_name(format!("{name}.{ext}"));
                    if target != p {
                        if target.exists() {
                            self.toast("A clip with that name already exists");
                            self.renaming = Some((p, name));
                            return;
                        }
                        match std::fs::rename(&p, &target) {
                            Ok(()) => crate::clipmeta::renamed(&p, &target),
                            Err(e) => self.toast(format!("Couldn't rename: {e}")),
                        }
                    }
                    self.lib.scan(&self.cfg.clips_dir);
                }
            }
        }
    }

    // ----------------------------------------------------------- settings --

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label(RichText::new("Changes apply instantly.").size(12.5).color(TEXT_FAINT));
        ui.add_space(4.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(760.0));
            let mut changed = false;

            section_title(ui, "Clipping");
            card(ui, |ui| {
                setting_row(ui, "Clip length", "How much of the past is saved", |ui| {
                    let opts: Vec<(u32, String)> = [300, 120, 60, 30, 15].iter().map(|&s| (s, fmt_secs(s))).collect();
                    changed |= chips(ui, &mut self.cfg.clip_seconds, &opts);
                });
                divider(ui);
                setting_row(ui, "Save hotkey", "Press again within 10 s to make the clip longer", |ui| {
                    let label = if self.capturing_hotkey { "Press a key combination…".to_string() } else { self.cfg.hotkey.label() };
                    let b = egui::Button::new(RichText::new(label).family(bold()).size(13.5).color(if self.capturing_hotkey {
                        Color32::BLACK
                    } else {
                        TEXT
                    }))
                    .fill(if self.capturing_hotkey { ACCENT } else { CONTROL })
                    .stroke(egui::Stroke::new(1.0_f32, BORDER))
                    .rounding(6.0)
                    .min_size(Vec2::new(200.0, 32.0));
                    if ui.add(b).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.capturing_hotkey = !self.capturing_hotkey;
                        self.hotkey_hint = None;
                    }
                });
                if let Some(h) = &self.hotkey_hint {
                    ui.label(RichText::new(h).size(12.0).color(AMBER));
                } else if self.capturing_hotkey {
                    ui.label(RichText::new("Hold Ctrl / Alt / Shift and press a key. Esc cancels.").size(12.0).color(TEXT_DIM));
                }
            });

            section_title(ui, "Video");
            card(ui, |ui| {
                setting_row(ui, "Monitor", "Which screen is recorded", |ui| {
                    let sel = self
                        .monitors
                        .get(self.cfg.monitor as usize)
                        .map(|m| m.0.clone())
                        .unwrap_or_else(|| "Primary monitor".into());
                    egui::ComboBox::from_id_salt("monitor").selected_text(sel).width(260.0).show_ui(ui, |ui| {
                        for (i, m) in self.monitors.iter().enumerate() {
                            if ui.selectable_value(&mut self.cfg.monitor, i as u32, &m.0).changed() {
                                changed = true;
                            }
                        }
                    });
                });
                divider(ui);
                setting_row(ui, "Resolution", "Lower = smaller files", |ui| {
                    let opts: Vec<(Resolution, String)> = Resolution::ALL.iter().rev().map(|r| (*r, r.label().to_string())).collect();
                    changed |= chips(ui, &mut self.cfg.resolution, &opts);
                });
                divider(ui);
                setting_row(ui, "Frame rate", "Frames per second", |ui| {
                    let opts: Vec<(u32, String)> = [144, 120, 60, 30].iter().map(|&f| (f, f.to_string())).collect();
                    changed |= chips(ui, &mut self.cfg.fps, &opts);
                });
                divider(ui);
                setting_row(ui, "Codec", "AV1 / HEVC: ~35% smaller, some apps can't play them", |ui| {
                    let opts: Vec<(Codec, String)> = Codec::ALL.iter().rev().map(|c| (*c, c.label().to_string())).collect();
                    changed |= chips(ui, &mut self.cfg.codec, &opts);
                });
                divider(ui);
                setting_row(ui, "Quality", "Higher = sharper, larger files", |ui| {
                    let opts: Vec<(Quality, String)> = Quality::ALL.iter().rev().map(|q| (*q, q.label().to_string())).collect();
                    changed |= chips(ui, &mut self.cfg.quality, &opts);
                });
                let (mw, mh) = self.monitors.get(self.cfg.monitor as usize).map(|m| (m.1, m.2)).unwrap_or((1920, 1080));
                let (ow, oh) = self.cfg.resolution.output_size(mw, mh);
                let br = self.cfg.video_bitrate(ow, oh, self.cfg.codec);
                let ram = (br as f64 / 8.0 + 24_000.0) * (self.cfg.clip_seconds + 12) as f64;
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{ow}×{oh} · ~{} Mbps · about {} per clip · uses ~{} of RAM · encoded on your GPU",
                        (br + 500_000) / 1_000_000,
                        fmt_size((ram * self.cfg.clip_seconds as f64 / (self.cfg.clip_seconds + 12) as f64) as u64),
                        fmt_size(ram as u64)
                    ))
                    .size(12.0)
                    .color(TEXT_FAINT),
                );
            });

            section_title(ui, "Audio");
            card(ui, |ui| {
                setting_row(ui, "System audio", "Game, music, voice chat — everything you hear", |ui| {
                    changed |= toggle(ui, &mut self.cfg.system_audio).changed();
                });
                divider(ui);
                setting_row(ui, "Microphone", "Mixed in with system audio (default mic)", |ui| {
                    changed |= toggle(ui, &mut self.cfg.microphone).changed();
                });
            });

            section_title(ui, "Saving");
            card(ui, |ui| {
                setting_row(ui, "Clips folder", "Type a path or browse", |ui| {
                    if button(ui, "Browse…").clicked() {
                        if let Some(p) = win::pick_folder(&self.cfg.clips_dir) {
                            self.folder_edit = p.display().to_string();
                            self.cfg.clips_dir = p;
                            changed = true;
                            self.lib.scan(&self.cfg.clips_dir);
                        }
                    }
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.folder_edit)
                            .desired_width((ui.available_width() - 8.0).clamp(160.0, 320.0))
                            .hint_text("E:\\videos"),
                    );
                    let commit = r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                    if commit {
                        let p = PathBuf::from(self.folder_edit.trim());
                        if p.as_os_str().is_empty() {
                            self.folder_edit = self.cfg.clips_dir.display().to_string();
                        } else if p != self.cfg.clips_dir {
                            match std::fs::create_dir_all(&p) {
                                Ok(()) => {
                                    self.cfg.clips_dir = p;
                                    changed = true;
                                    self.lib.scan(&self.cfg.clips_dir);
                                    self.toast("Clips folder updated");
                                }
                                Err(e) => {
                                    self.toast(format!("Can't use that folder: {e}"));
                                    self.folder_edit = self.cfg.clips_dir.display().to_string();
                                }
                            }
                        }
                    }
                });
                divider(ui);
                setting_row(ui, "Sort into game folders", "Uses your existing folders, e.g. E:\\videos\\Valorant", |ui| {
                    changed |= toggle(ui, &mut self.cfg.game_folders).changed();
                });
                if self.cfg.game_folders {
                    divider(ui);
                    setting_row(ui, "Create folders for new games", "Off: clips of unknown games go in the main folder", |ui| {
                        changed |= toggle(ui, &mut self.cfg.new_game_folders).changed();
                    });
                }
                divider(ui);
                setting_row(ui, "Copy new clips automatically", "Copies the full-size file after each save", |ui| {
                    changed |= toggle(ui, &mut self.cfg.copy_after_save).changed();
                });
                divider(ui);
                setting_row(ui, "Sound when saved", "Your confirmation, even in fullscreen games", |ui| {
                    changed |= toggle(ui, &mut self.cfg.sound_on_save).changed();
                });
                if self.cfg.sound_on_save {
                    divider(ui);
                    setting_row(ui, "Sound", "Click one to hear it", |ui| {
                        let opts: Vec<(crate::sound::SaveSound, String)> =
                            crate::sound::SaveSound::ALL.iter().rev().map(|s| (*s, s.label().to_string())).collect();
                        if chips(ui, &mut self.cfg.save_sound, &opts) {
                            changed = true;
                            crate::sound::play(self.cfg.save_sound, self.cfg.save_volume);
                        }
                    });
                    divider(ui);
                    setting_row(ui, "Volume", "Plays a preview when you let go", |ui| {
                        if button(ui, "Play").clicked() {
                            crate::sound::play(self.cfg.save_sound, self.cfg.save_volume);
                        }
                        ui.add_space(6.0);
                        ui.spacing_mut().slider_width = 180.0;
                        let r = ui.add(egui::Slider::new(&mut self.cfg.save_volume, 0..=100).suffix(" %"));
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            changed = true;
                            crate::sound::play(self.cfg.save_sound, self.cfg.save_volume);
                        }
                    });
                }
                divider(ui);
                setting_row(ui, "Notification when saved", "Errors are always shown", |ui| {
                    changed |= toggle(ui, &mut self.cfg.notify_on_save).changed();
                });
            });

            section_title(ui, "Sharing");
            card(ui, |ui| {
                setting_row(ui, "Discord upload limit", "Free 10 MB · Nitro Basic 50 MB · Nitro 500 MB", |ui| {
                    let opts: Vec<(u32, String)> = [500, 50, 10].iter().map(|&m| (m, format!("{m} MB"))).collect();
                    changed |= chips(ui, &mut self.cfg.discord_limit_mb, &opts);
                });
                divider(ui);
                setting_row(ui, "Custom size", "Any size you like: drag, or click and type", |ui| {
                    let r = ui.add(
                        egui::DragValue::new(&mut self.cfg.discord_limit_mb)
                            .range(1..=2000)
                            .speed(1.0)
                            .suffix(" MB"),
                    );
                    // Save when the drag ends or typing finishes, not on every step.
                    if r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus()) {
                        changed = true;
                    }
                });
                divider(ui);
                setting_row(ui, "Copy fits Discord", "Copy shrinks clips over the limit first", |ui| {
                    changed |= toggle(ui, &mut self.cfg.copy_fits_discord).changed();
                });
                divider(ui);
                setting_row(ui, "Copy after shrinking", "Ready to paste into Discord", |ui| {
                    changed |= toggle(ui, &mut self.cfg.copy_after_shrink).changed();
                });
                ui.add_space(2.0);
                ui.label(
                    RichText::new("The small version is saved next to the clip as \"<name> (10 MB).mp4\" and reused next time. For a one-off size, use a clip's ··· menu, Shrink to size.")
                        .size(12.0)
                        .color(TEXT_FAINT),
                );
            });

            section_title(ui, "Cleanup");
            let before = (self.cfg.cleanup_enabled, self.cfg.cleanup_max_gb, self.cfg.cleanup_max_days, self.cfg.clips_dir.clone());
            card(ui, |ui| {
                setting_row(ui, "Auto-delete old clips", "Only ezclip's clips. Favourites are kept", |ui| {
                    changed |= toggle(ui, &mut self.cfg.cleanup_enabled).changed();
                });
                divider(ui);
                setting_row(ui, "Keep clips under", "Oldest clips go first", |ui| {
                    let opts: Vec<(u32, String)> = [500, 250, 100, 50, 25, 0]
                        .iter()
                        .map(|&g| (g, if g == 0 { "No limit".to_string() } else { format!("{g} GB") }))
                        .collect();
                    changed |= chips(ui, &mut self.cfg.cleanup_max_gb, &opts);
                });
                divider(ui);
                setting_row(ui, "Delete clips older than", "", |ui| {
                    let opts: Vec<(u32, String)> = [(365, "1 year"), (90, "90 days"), (30, "30 days"), (7, "7 days"), (0, "Never")]
                        .iter()
                        .map(|&(d, l)| (d, l.to_string()))
                        .collect();
                    changed |= chips(ui, &mut self.cfg.cleanup_max_days, &opts);
                });
                divider(ui);
                setting_row(ui, "Use the Recycle Bin", "Recoverable; frees space once emptied", |ui| {
                    changed |= toggle(ui, &mut self.cfg.cleanup_recycle).changed();
                });
                let after = (self.cfg.cleanup_enabled, self.cfg.cleanup_max_gb, self.cfg.cleanup_max_days, self.cfg.clips_dir.clone());
                if after != before {
                    self.cleanup_preview = None;
                }
                let (text, warn) = self.cleanup_preview.get_or_insert_with(|| cleanup_summary(&self.cfg)).clone();
                ui.add_space(2.0);
                ui.label(RichText::new(text).size(12.0).color(if warn { AMBER } else { TEXT_FAINT }));
            });

            section_title(ui, "General");
            card(ui, |ui| {
                setting_row(ui, "Start with Windows", "Runs quietly in the tray after you sign in", |ui| {
                    let mut on = self.startup;
                    if toggle(ui, &mut on).changed() {
                        if win::set_startup(on) {
                            self.startup = on;
                        } else {
                            self.toast("Couldn't change the startup setting");
                        }
                    }
                });
                divider(ui);
                setting_row(
                    ui,
                    "Background recorder",
                    if self.recorder_running { "Running in the system tray" } else { "Stopped" },
                    |ui| {
                        if self.recorder_running {
                            if button(ui, "Stop").clicked() {
                                win::post_to_recorder(win::WM_EZ_QUIT);
                                self.last_status = Instant::now() - Duration::from_millis(700);
                            }
                        } else if button(ui, "Start").clicked() {
                            win::spawn_self("--background");
                            self.last_status = Instant::now() - Duration::from_millis(700);
                        }
                    },
                );
                divider(ui);
                setting_row(ui, "Version", concat!("ezclip ", env!("CARGO_PKG_VERSION")), |ui| {
                    if button(ui, "Releases").clicked() {
                        win::open_url("https://github.com/Tsumikaze/ezclip/releases");
                    }
                });
                divider(ui);
                setting_row(ui, "Logs", "For troubleshooting", |ui| {
                    if button(ui, "Open").clicked() {
                        win::open_path(&crate::config::app_dir());
                    }
                });
            });
            ui.add_space(24.0);

            if changed {
                self.apply();
            }
        });
    }

    fn editor_page(&mut self, ui: &mut egui::Ui) {
        let (Some(ed), Some(grabber)) = (self.editor.as_mut(), self.grabber.as_ref()) else {
            self.page = Page::Clips;
            return;
        };
        match ed.show(ui, grabber) {
            EditorAction::None => {}
            EditorAction::Back => {
                self.editor = None;
                self.page = Page::Clips;
            }
            act => {
                let replace = matches!(act, EditorAction::Replace(..));
                let (EditorAction::SaveCopy(start, end) | EditorAction::Replace(start, end)) = act else { return };
                ed.busy = true;
                let src = ed.path.clone();
                let tx = self.trim_tx.clone();
                let ctx = self.ctx.clone();
                std::thread::spawn(move || {
                    let result = trim_job(&src, start, end, replace);
                    let _ = tx.send(result.map(|p| (p, replace)));
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Start decoding hover frames for a clip (once).
    fn request_hover_frames(&mut self, path: &PathBuf) {
        if self.hover_frames.contains_key(path) || self.hover_failed.contains(path) || self.hover_requested.as_ref() == Some(path) {
            return;
        }
        // Keep memory small: forget other clips' frames past a few dozen clips.
        if self.hover_frames.len() > 24 {
            self.hover_frames.clear();
        }
        self.hover_requested = Some(path.clone());
        let ctx = self.ctx.clone();
        self.hover.get_or_insert_with(|| HoverGrabber::new(ctx)).request(path.clone());
    }

    fn pump_hover(&mut self) {
        let Some(h) = &self.hover else { return };
        for f in h.poll() {
            match f.index {
                None => {
                    self.hover_failed.insert(f.path.clone());
                }
                Some(i) => {
                    let tex = f.image.map(|img| self.ctx.load_texture(format!("hover-{}-{i}", f.path.display()), img, egui::TextureOptions::LINEAR));
                    let slots = self.hover_frames.entry(f.path.clone()).or_insert_with(|| vec![None; HOVER_FRAMES]);
                    slots[i] = tex;
                }
            }
            if self.hover_requested.as_ref() == Some(&f.path) && (f.index.is_none() || f.index == Some(HOVER_FRAMES - 1)) {
                self.hover_requested = None;
            }
        }
    }

    /// Results from background trim / shrink jobs.
    fn pump_jobs(&mut self) {
        while let Ok(r) = self.trim_rx.try_recv() {
            match r {
                Ok((_, replaced)) => {
                    self.toast(if replaced { "Clip trimmed" } else { "Trimmed copy saved" });
                    self.editor = None;
                    self.page = Page::Clips;
                }
                Err(e) => {
                    if let Some(ed) = self.editor.as_mut() {
                        ed.busy = false;
                    }
                    self.toast(format!("Couldn't trim: {e}"));
                }
            }
            self.lib.scan(&self.cfg.clips_dir);
        }
        while let Ok(u) = self.shrink_rx.try_recv() {
            match u {
                ShrinkUpdate::Progress(p, f) => {
                    self.shrinking.insert(p, f);
                }
                ShrinkUpdate::Done(p, r) => {
                    self.shrinking.remove(&p);
                    let mb = self.shrink_limits.remove(&p).unwrap_or(self.cfg.discord_limit_mb);
                    let want_copy = self.copy_when_shrunk.remove(&p) || self.cfg.copy_after_shrink;
                    match r {
                        Ok(out) => {
                            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                            let copied = want_copy && win::copy_files_to_clipboard(&[&out]);
                            let what = if out == p {
                                format!("Already under {mb} MB")
                            } else {
                                format!("Shrunk to {}", fmt_size(size))
                            };
                            self.toast(if copied { format!("{what} — copied, paste into Discord with Ctrl+V") } else { what });
                        }
                        Err(e) => self.toast(format!("Couldn't shrink: {e}")),
                    }
                    self.lib.scan(&self.cfg.clips_dir);
                }
            }
        }
    }

    fn capture_hotkey(&mut self, ctx: &egui::Context) {
        if !self.capturing_hotkey {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                if key == Key::Escape {
                    self.capturing_hotkey = false;
                    self.hotkey_hint = None;
                    return;
                }
                let Some(vk) = key_to_vk(key) else { continue };
                let has_mod = modifiers.ctrl || modifiers.alt || modifiers.shift;
                let standalone_ok = matches!(vk, 0x70..=0x87 | 0x13 | 0x91 | 0x2D | 0x24 | 0x23 | 0x21 | 0x22);
                if !has_mod && !standalone_ok {
                    self.hotkey_hint = Some(format!("{} alone would block typing — add Ctrl, Alt or Shift.", vk_name(vk)));
                    continue;
                }
                self.cfg.hotkey = Hotkey { ctrl: modifiers.ctrl, alt: modifiers.alt, shift: modifiers.shift, vk };
                self.capturing_hotkey = false;
                self.hotkey_hint = None;
                self.apply();
                self.toast(format!("Hotkey set to {}", self.cfg.hotkey.label()));
            }
        }
    }
}

/// The full clip menu, used by the "···" button and right-click.
fn clip_menu(ui: &mut egui::Ui, path: &std::path::Path, favorite: bool, limit_mb: u32) -> Option<ClipAction> {
    let mut act = None;
    let p = || path.to_path_buf();
    if ui.button("Play").clicked() {
        act = Some(ClipAction::Play(p()));
    }
    if ui.button("Copy for Discord").clicked() {
        act = Some(ClipAction::Copy(p()));
    }
    if ui.button("Copy original file").clicked() {
        act = Some(ClipAction::CopyOriginal(p()));
    }
    if ui.button("Trim…").clicked() {
        act = Some(ClipAction::Trim(p()));
    }
    if ui.button(format!("Shrink for Discord ({limit_mb} MB)")).clicked() {
        act = Some(ClipAction::Shrink(p(), limit_mb));
    }
    ui.menu_button("Shrink to size", |ui| {
        let mut sizes = vec![10u32, 25, 50, 100, 250, 500];
        if !sizes.contains(&limit_mb) {
            sizes.push(limit_mb);
            sizes.sort();
        }
        for mb in sizes {
            if ui.button(format!("{mb} MB")).clicked() {
                act = Some(ClipAction::Shrink(p(), mb));
            }
        }
    });
    if ui.button(if favorite { "Remove from favourites" } else { "Add to favourites" }).clicked() {
        act = Some(ClipAction::Favorite(p(), !favorite));
    }
    if ui.button("Rename").clicked() {
        act = Some(ClipAction::StartRename(p()));
    }
    if ui.button("Show in folder").clicked() {
        act = Some(ClipAction::Reveal(p()));
    }
    ui.separator();
    if ui.button(RichText::new("Delete (to Recycle Bin)").color(RED)).clicked() {
        act = Some(ClipAction::Delete(p()));
    }
    if act.is_some() {
        ui.close_menu();
    }
    act
}

/// Painted star toggle (no font glyph needed).
fn star_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::click());
    let c = rect.center();
    let mut pts = Vec::with_capacity(10);
    for k in 0..10 {
        let r = if k % 2 == 0 { 8.5 } else { 3.6 };
        let a = std::f32::consts::PI * (k as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
        pts.push(c + Vec2::new(a.cos() * r, a.sin() * r));
    }
    let hover = resp.hovered();
    let color = if on { Color32::from_rgb(255, 204, 64) } else if hover { TEXT } else { TEXT_DIM };
    // Fill as triangles from the centre (the star isn't convex).
    let painter = ui.painter();
    for k in 0..10 {
        let a = pts[k];
        let b = pts[(k + 1) % 10];
        if on {
            painter.add(egui::Shape::convex_polygon(vec![c, a, b], color, egui::Stroke::NONE));
        } else {
            painter.line_segment([a, b], egui::Stroke::new(1.7_f32, color));
        }
    }
    resp.on_hover_text(if on { "Favourite: never auto-deleted" } else { "Add to favourites" })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

enum ClipAction {
    Trim(PathBuf),
    Shrink(PathBuf, u32),
    Favorite(PathBuf, bool),
    Play(PathBuf),
    Copy(PathBuf),
    CopyOriginal(PathBuf),
    Reveal(PathBuf),
    Delete(PathBuf),
    StartRename(PathBuf),
    CommitRename,
    CancelRename,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_status.elapsed() > Duration::from_millis(1000) {
            self.poll_status();
        }
        if self.page == Page::Clips && self.last_scan.elapsed() > Duration::from_secs(2) && self.renaming.is_none() {
            self.last_scan = Instant::now();
            self.lib.scan(&self.cfg.clips_dir);
        }
        self.lib.pump(ctx);
        self.pump_jobs();
        self.pump_hover();
        self.capture_hotkey(ctx);
        // Light polling only while the window is open.
        ctx.request_repaint_after(Duration::from_millis(1000));

        egui::SidePanel::left("nav")
            .exact_width(230.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(18.0))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(20, 20, 20))),
            )
            .show(ctx, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin { left: 32.0, right: 28.0, top: 26.0, bottom: 10.0 }))
            .show(ctx, |ui| match self.page {
                Page::Clips => self.clips_page(ui),
                Page::Settings => self.settings_page(ui),
                Page::Editor => self.editor_page(ui),
            });

        // Toast
        if let Some((msg, t)) = &self.toast {
            if t.elapsed() < Duration::from_millis(2600) {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(115.0, -24.0))
                    .show(ctx, |ui| {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(28, 28, 28))
                            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(50, 50, 50)))
                            .rounding(R)
                            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
                            .show(ui, |ui| ui.label(RichText::new(msg).size(13.0)));
                    });
                ctx.request_repaint_after(Duration::from_millis(200));
            } else {
                self.toast = None;
            }
        }
    }
}

/// One-line preview of what auto-cleanup does with the current settings.
fn cleanup_summary(cfg: &Config) -> (String, bool) {
    let plan = crate::cleanup::plan(cfg);
    let used = format!("Clips made by ezclip use {}.", fmt_size(plan.ezclip_bytes));
    let limits_off = cfg.cleanup_max_gb == 0 && cfg.cleanup_max_days == 0;
    if limits_off {
        return (format!("{used} No limits set."), false);
    }
    if plan.remove.is_empty() {
        return (format!("{used} Nothing would be removed right now."), false);
    }
    let what = format!("{} clip{} ({})", plan.remove.len(), if plan.remove.len() == 1 { "" } else { "s" }, fmt_size(plan.bytes()));
    if cfg.cleanup_enabled {
        (format!("{used} {what} will be removed now."), true)
    } else {
        (format!("{used} Turning this on would remove {what} right away."), true)
    }
}

/// Lossless trim on a background thread. A copy goes next to the original as
/// "<name> (trim).mp4"; replacing keeps the original's favourite/ezclip flags.
fn trim_job(src: &std::path::Path, start: f64, end: f64, replace: bool) -> Result<PathBuf, String> {
    use crate::clipmeta;
    if replace {
        let fav = clipmeta::is_favorite(src);
        let ours = clipmeta::is_ezclip(src);
        crate::clipfile::trim(src, src, start, end)?;
        if fav {
            clipmeta::set_favorite(src, true);
        }
        if ours {
            clipmeta::mark_ezclip(src);
        }
        Ok(src.to_path_buf())
    } else {
        let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut dst = src.with_file_name(format!("{stem} (trim).mp4"));
        let mut n = 2;
        while dst.exists() {
            dst = src.with_file_name(format!("{stem} (trim {n}).mp4"));
            n += 1;
        }
        crate::clipfile::trim(src, &dst, start, end)?;
        clipmeta::mark_ezclip(&dst);
        Ok(dst)
    }
}

// ---------------------------------------------------------------- format --

fn fmt_secs(s: u32) -> String {
    if s >= 60 && s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

fn fmt_duration(d: f64) -> String {
    let s = d.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn fmt_size(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn fmt_date(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    let today = chrono::Local::now().date_naive();
    let d = dt.date_naive();
    if d == today {
        format!("Today, {}", dt.format("%H:%M"))
    } else if Some(d) == today.pred_opt() {
        format!("Yesterday, {}", dt.format("%H:%M"))
    } else {
        dt.format("%b %-d, %Y · %H:%M").to_string()
    }
}

fn key_to_vk(k: Key) -> Option<u32> {
    use Key::*;
    const LETTERS: [Key; 26] = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    const DIGITS: [Key; 10] = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    const FKEYS: [Key; 24] = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    ];
    if let Some(i) = LETTERS.iter().position(|x| *x == k) {
        return Some(0x41 + i as u32);
    }
    if let Some(i) = DIGITS.iter().position(|x| *x == k) {
        return Some(0x30 + i as u32);
    }
    if let Some(i) = FKEYS.iter().position(|x| *x == k) {
        return Some(0x70 + i as u32);
    }
    Some(match k {
        Space => 0x20,
        Insert => 0x2D,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Minus => 0xBD,
        Equals => 0xBB,
        Backtick => 0xC0,
        OpenBracket => 0xDB,
        CloseBracket => 0xDD,
        Backslash => 0xDC,
        Semicolon => 0xBA,
        Quote => 0xDE,
        Comma => 0xBC,
        Period => 0xBE,
        Slash => 0xBF,
        _ => return None,
    })
}
