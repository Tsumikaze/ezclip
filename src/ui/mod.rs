//! The ezclip window: a Clips library and a Settings page.
//! Runs as its own short-lived process, so it costs nothing when closed.
mod clips;
mod theme;
mod widgets;

use crate::config::{vk_name, Config, Hotkey, Quality, Resolution, Status};
use crate::win;
use clips::Library;
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use theme::*;
use widgets::*;

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Clips,
    Settings,
}

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
        let mut app = Self {
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
        });
        ui.add_space(26.0);

        let count = self.lib.clips.len();
        for (page, label, badge) in [
            (Page::Clips, "Clips", if count > 0 { count.to_string() } else { String::new() }),
            (Page::Settings, "Settings", String::new()),
        ] {
            if self.nav_item(ui, label, &badge, self.page == page).clicked() {
                self.page = page;
                self.capturing_hotkey = false;
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
        ui.label(
            RichText::new(format!(
                "{} clip{} · {} · {}",
                n,
                if n == 1 { "" } else { "s" },
                fmt_size(self.lib.total_size()),
                self.cfg.clips_dir.display()
            ))
            .size(12.5)
            .color(TEXT_FAINT),
        );
        ui.add_space(14.0);

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

        let mut action: Option<ClipAction> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for i in 0..self.lib.clips.len() {
                if let Some(a) = self.clip_row(ui, i) {
                    action = Some(a);
                }
            }
            ui.add_space(8.0);
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

        // Thumbnail
        let (trect, tresp) = ui.allocate_exact_size(Vec2::new(thumb_w, row_h), Sense::click());
        match &c.thumb {
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
            let label = fmt_duration(d);
            let font = egui::FontId::new(11.5, bold());
            let galley = ui.painter().layout_no_wrap(label, font, Color32::WHITE);
            let br = egui::Rect::from_min_size(
                trect.right_bottom() - galley.size() - Vec2::new(12.0, 8.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            ui.painter().rect_filled(br, 4.0, Color32::from_black_alpha(200));
            ui.painter().galley(br.min + Vec2::new(4.0, 2.0), galley, Color32::WHITE);
        }
        if tresp.hovered() {
            ui.painter().circle_filled(trect.center(), 18.0, Color32::from_black_alpha(170));
            let c0 = trect.center();
            ui.painter().add(egui::Shape::convex_polygon(
                vec![c0 + Vec2::new(-5.0, -8.0), c0 + Vec2::new(9.0, 0.0), c0 + Vec2::new(-5.0, 8.0)],
                Color32::WHITE,
                egui::Stroke::NONE,
            ));
        }
        if tresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            act = Some(ClipAction::Play(path.clone()));
        }
        ui.add_space(14.0);

        // Title + meta
        let renaming_this = self.renaming.as_ref().is_some_and(|(p, _)| *p == path);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 200.0).max(120.0));
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
                        if let Some(a) = clip_menu(ui, &path) {
                            act = Some(a);
                        }
                    },
                );
                if r.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(ui.make_persistent_id(("more", &path))));
                }
            }
            if button(ui, "Copy").on_hover_text("Copy to paste into Discord, a browser or Explorer").clicked() {
                act = Some(ClipAction::Copy(path.clone()));
            }
            if button(ui, "Play").clicked() {
                act = Some(ClipAction::Play(path.clone()));
            }
        });

        // Right-click anywhere on the row: same menu.
        row_resp.context_menu(|ui| {
            ui.set_min_width(170.0);
            if let Some(a) = clip_menu(ui, &path) {
                act = Some(a);
            }
        });

        if row_resp.double_clicked() && act.is_none() {
            act = Some(ClipAction::Play(path));
        }
        act
    }

    fn do_action(&mut self, a: ClipAction) {
        match a {
            ClipAction::Play(p) => win::open_path(&p),
            ClipAction::Reveal(p) => win::show_in_folder(&p),
            ClipAction::Copy(p) => {
                if win::copy_files_to_clipboard(&[&p]) {
                    self.toast("Copied — paste it into Discord with Ctrl+V");
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
                        if let Err(e) = std::fs::rename(&p, &target) {
                            self.toast(format!("Couldn't rename: {e}"));
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
                setting_row(ui, "Quality", "Higher = sharper, larger files", |ui| {
                    let opts: Vec<(Quality, String)> = Quality::ALL.iter().rev().map(|q| (*q, q.label().to_string())).collect();
                    changed |= chips(ui, &mut self.cfg.quality, &opts);
                });
                let (mw, mh) = self.monitors.get(self.cfg.monitor as usize).map(|m| (m.1, m.2)).unwrap_or((1920, 1080));
                let (ow, oh) = self.cfg.resolution.output_size(mw, mh);
                let br = self.cfg.quality.bitrate(ow, oh, self.cfg.fps);
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
                setting_row(ui, "Copy new clips automatically", "Every saved clip is ready to paste (Ctrl+V)", |ui| {
                    changed |= toggle(ui, &mut self.cfg.copy_after_save).changed();
                });
                divider(ui);
                setting_row(ui, "Sound when saved", "A short, quiet chirp", |ui| {
                    changed |= toggle(ui, &mut self.cfg.sound_on_save).changed();
                });
                divider(ui);
                setting_row(ui, "Notification when saved", "Errors are always shown", |ui| {
                    changed |= toggle(ui, &mut self.cfg.notify_on_save).changed();
                });
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

/// Rename / show in folder / delete, used by the "···" button and right-click.
fn clip_menu(ui: &mut egui::Ui, path: &std::path::Path) -> Option<ClipAction> {
    let mut act = None;
    if ui.button("Play").clicked() {
        act = Some(ClipAction::Play(path.to_path_buf()));
    }
    if ui.button("Copy").clicked() {
        act = Some(ClipAction::Copy(path.to_path_buf()));
    }
    if ui.button("Rename").clicked() {
        act = Some(ClipAction::StartRename(path.to_path_buf()));
    }
    if ui.button("Show in folder").clicked() {
        act = Some(ClipAction::Reveal(path.to_path_buf()));
    }
    ui.separator();
    if ui.button(RichText::new("Delete (to Recycle Bin)").color(RED)).clicked() {
        act = Some(ClipAction::Delete(path.to_path_buf()));
    }
    if act.is_some() {
        ui.close_menu();
    }
    act
}

enum ClipAction {
    Play(PathBuf),
    Copy(PathBuf),
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
