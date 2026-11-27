//! Trim editor: preview with playback (picture + sound), a timeline with
//! start/end handles, and a lossless save. The start snaps to keyframes (that
//! is what makes the cut instant and lossless); the end is frame-accurate.
use super::media::{FrameGrabber, FrameRequest};
use super::player::{Player, PlayerEvent};
use super::theme::*;
use super::widgets::*;
use crate::clipfile::{self, ClipInfo};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, Vec2};
use std::path::PathBuf;

pub enum EditorAction {
    None,
    Back,
    SaveCopy(f64, f64),
    Replace(f64, f64),
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Start,
    End,
    Cursor,
}

pub struct Editor {
    pub path: PathBuf,
    name: String,
    info: Result<ClipInfo, String>,
    start: f64,
    end: f64,
    cursor: f64,
    frame: Option<egui::TextureHandle>,
    next_id: u64,
    shown_id: u64,
    drag: Option<Drag>,
    pub busy: bool,
    /// Running playback, if any.
    player: Option<Player>,
}

impl Editor {
    pub fn open(path: PathBuf, grabber: &FrameGrabber) -> Self {
        let info = clipfile::probe(&path);
        let end = info.as_ref().map(|i| i.duration).unwrap_or(0.0);
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut e = Self { path, name, info, start: 0.0, end, cursor: 0.0, frame: None, next_id: 0, shown_id: 0, drag: None, busy: false, player: None };
        e.request(grabber, 0.0, false);
        e
    }

    fn duration(&self) -> f64 {
        self.info.as_ref().map(|i| i.duration).unwrap_or(0.0)
    }

    fn snap_start(&self, t: f64) -> f64 {
        match &self.info {
            Ok(i) => i.keyframes.iter().copied().filter(|&k| k <= t + 1e-6).last().unwrap_or(0.0),
            Err(_) => 0.0,
        }
    }

    fn play(&mut self, ctx: &egui::Context, from: f64, until: Option<f64>) {
        self.player = Some(Player::start(ctx.clone(), self.path.clone(), from, until));
    }

    fn pause(&mut self) {
        self.player = None;
        // Frames the grabber decoded before playback are stale now.
        self.shown_id = self.next_id + 1;
    }

    /// Play from the playhead (or from the start if it's at the very end).
    fn toggle_play(&mut self, ctx: &egui::Context) {
        if self.player.is_some() {
            self.pause();
        } else {
            let from = if self.cursor >= self.duration() - 0.1 { self.start } else { self.cursor };
            self.play(ctx, from, None);
        }
    }

    fn request(&mut self, grabber: &FrameGrabber, t: f64, exact: bool) {
        self.next_id += 1;
        grabber.request(FrameRequest { path: self.path.clone(), seconds: t, exact, id: self.next_id });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, grabber: &FrameGrabber) -> EditorAction {
        let mut action = EditorAction::None;
        let ctx = ui.ctx().clone();
        // Playback frames move the playhead; scrub frames only show when paused.
        let mut ended = false;
        if let Some(p) = &self.player {
            for ev in p.poll() {
                match ev {
                    PlayerEvent::Frame(t, img) => {
                        self.cursor = t;
                        match &mut self.frame {
                            Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                            None => self.frame = Some(ctx.load_texture("trim-frame", img, egui::TextureOptions::LINEAR)),
                        }
                    }
                    PlayerEvent::Ended => ended = true,
                }
            }
        }
        if ended {
            self.pause();
        }
        let grabbed = grabber.poll();
        if self.player.is_none() {
            if let Some(f) = grabbed.filter(|f| f.id >= self.shown_id) {
                self.shown_id = f.id;
                if let Some(img) = f.image {
                    self.frame = Some(ctx.load_texture("trim-frame", img, egui::TextureOptions::LINEAR));
                }
            }
        }

        ui.horizontal(|ui| {
            if button(ui, "Back").clicked() {
                action = EditorAction::Back;
            }
            ui.add_space(6.0);
            ui.heading("Trim");
            ui.add(egui::Label::new(RichText::new(&self.name).size(14.0).color(TEXT_DIM)).truncate());
        });
        ui.add_space(12.0);

        if let Err(e) = &self.info {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("This clip can't be trimmed").size(17.0).family(bold()));
                ui.label(RichText::new(e).color(TEXT_DIM));
            });
            return action;
        }
        let dur = self.duration().max(0.001);

        // Preview
        let w = ui.available_width();
        let h = (w * 9.0 / 16.0).min(ui.available_height() - 150.0).max(120.0);
        let (prect, presp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        ui.painter().rect_filled(prect, R, Color32::from_rgb(8, 8, 8));
        if let Some(t) = &self.frame {
            let size = t.size_vec2();
            let scale = (prect.width() / size.x).min(prect.height() / size.y);
            let r = egui::Rect::from_center_size(prect.center(), size * scale);
            egui::Image::new(t).rounding(6.0).paint_at(ui, r);
        }
        if self.player.is_none() && presp.hovered() && !self.busy {
            // Big play icon over the paused picture.
            let c = prect.center();
            ui.painter().circle_filled(c, 30.0, Color32::from_black_alpha(160));
            paint_play_icon(ui.painter(), c, 14.0, Color32::WHITE);
        }
        if presp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !self.busy {
            self.toggle_play(&ctx);
        }
        ui.add_space(10.0);

        // Timeline
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0), Sense::click_and_drag());
        let bar = egui::Rect::from_min_max(egui::pos2(rect.left() + 6.0, rect.top() + 6.0), egui::pos2(rect.right() - 6.0, rect.bottom() - 6.0));
        let x_of = |t: f64| bar.left() + (t / dur) as f32 * bar.width();
        let t_of = |x: f32| (((x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64) * dur;
        let p = ui.painter();
        p.rect_filled(bar, 6.0, Color32::from_rgb(22, 22, 22));
        let sel = egui::Rect::from_min_max(egui::pos2(x_of(self.start), bar.top()), egui::pos2(x_of(self.end), bar.bottom()));
        p.rect_filled(sel, 4.0, Color32::from_rgb(58, 58, 58));
        if let Ok(info) = &self.info {
            for &k in &info.keyframes {
                let x = x_of(k);
                p.line_segment([egui::pos2(x, bar.bottom() - 6.0), egui::pos2(x, bar.bottom() - 2.0)], egui::Stroke::new(1.0_f32, TEXT_FAINT));
            }
        }
        for t in [self.start, self.end] {
            let x = x_of(t);
            let hr = egui::Rect::from_center_size(egui::pos2(x, bar.center().y), Vec2::new(7.0, bar.height() + 6.0));
            p.rect_filled(hr, 3.0, ACCENT);
        }
        let cx = x_of(self.cursor);
        p.line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())], egui::Stroke::new(2.0_f32, RED));

        if let Some(pos) = resp.interact_pointer_pos() {
            if self.player.is_some() && (resp.drag_started() || resp.clicked()) {
                self.pause(); // touching the timeline pauses playback
            }
            if resp.drag_started() || (resp.clicked() && self.drag.is_none()) {
                let ds = (pos.x - x_of(self.start)).abs();
                let de = (pos.x - x_of(self.end)).abs();
                self.drag = Some(if ds < 12.0 && ds <= de {
                    Drag::Start
                } else if de < 12.0 {
                    Drag::End
                } else {
                    Drag::Cursor
                });
            }
            if resp.dragged() || resp.clicked() {
                let t = t_of(pos.x);
                match self.drag {
                    Some(Drag::Start) => {
                        let s = self.snap_start(t.min(self.end - 0.5));
                        if s != self.start {
                            self.start = s;
                            self.cursor = s;
                            self.request(grabber, s, false);
                        }
                    }
                    Some(Drag::End) => {
                        let e = t.max(self.start + 0.5).min(dur);
                        if (e - self.end).abs() > 1e-3 {
                            self.end = e;
                            self.cursor = e;
                            self.request(grabber, e, true);
                        }
                    }
                    _ => {
                        self.cursor = t;
                        self.request(grabber, t, true);
                    }
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            self.drag = None;
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);

        // Space plays / pauses; I / O set start / end at the playhead.
        if !self.busy {
            let (i, o, space) = ui.input(|inp| (inp.key_pressed(Key::I), inp.key_pressed(Key::O), inp.key_pressed(Key::Space)));
            if space {
                self.toggle_play(&ctx);
            }
            if i {
                self.start = self.snap_start(self.cursor.min(self.end - 0.5));
                self.request(grabber, self.start, false);
            }
            if o {
                self.end = self.cursor.max(self.start + 0.5).min(dur);
            }
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let playing = self.player.is_some();
            if play_button(ui, playing).on_hover_text(if playing { "Pause (Space)" } else { "Play from the playhead (Space)" }).clicked() && !self.busy {
                self.toggle_play(&ctx);
            }
            if button(ui, "Play selection").on_hover_text("Plays exactly what the trimmed clip will contain").clicked() && !self.busy {
                self.cursor = self.start;
                self.play(&ctx, self.start, Some(self.end));
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{}  –  {}    ·    {:.1} s",
                    fmt_t(self.start),
                    fmt_t(self.end),
                    self.end - self.start
                ))
                .size(15.0)
                .family(bold()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.busy {
                    ui.spinner();
                    ui.label(RichText::new("Saving…").color(TEXT_DIM));
                    return;
                }
                if primary_button(ui, "Save as new clip", 170.0).clicked() {
                    action = EditorAction::SaveCopy(self.start, self.end);
                }
                if button(ui, "Replace original").clicked() {
                    action = EditorAction::Replace(self.start, self.end);
                }
            });
        });
        ui.label(
            RichText::new("Drag the white handles. The start snaps to keyframes, so trimming is instant and lossless. Space plays / pauses, I / O set start / end at the playhead.")
                .size(12.0)
                .color(TEXT_FAINT),
        );
        action
    }
}

fn paint_play_icon(p: &egui::Painter, c: egui::Pos2, size: f32, color: Color32) {
    let pts = vec![c + Vec2::new(-size * 0.45, -size * 0.6), c + Vec2::new(size * 0.65, 0.0), c + Vec2::new(-size * 0.45, size * 0.6)];
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// White round play / pause button with a painted icon (no font glyphs needed).
fn play_button(ui: &mut egui::Ui, playing: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(38.0, 38.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { Color32::from_rgb(225, 225, 225) } else { ACCENT };
    p.circle_filled(rect.center(), 18.0, fill);
    let c = rect.center();
    if playing {
        for dx in [-4.0, 4.0] {
            p.rect_filled(egui::Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(4.0, 13.0)), 1.0, Color32::BLACK);
        }
    } else {
        paint_play_icon(p, c + Vec2::new(1.5, 0.0), 12.0, Color32::BLACK);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn fmt_t(t: f64) -> String {
    let m = (t / 60.0).floor() as u64;
    format!("{}:{:04.1}", m, t - m as f64 * 60.0)
}
