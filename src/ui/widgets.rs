//! Custom controls: chips (segmented choice), toggle switch, buttons.
use super::theme::*;
use eframe::egui::{self, Color32, Response, RichText, Sense, Stroke, Ui, Vec2};

/// Pill buttons added into the current layout (in the order given); returns
/// true when the value changed. Inside a right-to-left layout, pass the
/// options in reverse so they read left-to-right on screen.
pub fn chips<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, String)]) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 6.0;
    for (v, label) in options {
        let selected = *value == *v;
        let (fill, text) = if selected { (ACCENT, Color32::BLACK) } else { (CONTROL, TEXT_DIM) };
        let btn = egui::Button::new(RichText::new(label).color(text).family(bold()).size(13.0))
            .fill(fill)
            .stroke(if selected { Stroke::NONE } else { Stroke::new(1.0_f32, BORDER) })
            .rounding(egui::Rounding::same(15.0))
            .min_size(Vec2::new(52.0, 30.0));
        let r = ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// A single on/off pill (e.g. the Favourites filter).
pub fn chip_toggle(ui: &mut Ui, label: &str, on: bool) -> Response {
    let (fill, text) = if on { (ACCENT, Color32::BLACK) } else { (CONTROL, TEXT_DIM) };
    let btn = egui::Button::new(RichText::new(label).color(text).family(bold()).size(13.0))
        .fill(fill)
        .stroke(if on { Stroke::NONE } else { Stroke::new(1.0_f32, BORDER) })
        .rounding(egui::Rounding::same(15.0))
        .min_size(Vec2::new(52.0, 30.0));
    ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(42.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let bg = lerp_color(Color32::from_rgb(45, 45, 45), ACCENT, t);
    let knob = lerp_color(Color32::from_rgb(160, 160, 160), Color32::BLACK, t);
    let p = ui.painter();
    p.rect_filled(rect, egui::Rounding::same(12.0), bg);
    let r = 8.0;
    let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    p.circle_filled(egui::pos2(x, rect.center().y), r, knob);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// White, prominent button.
pub fn primary_button(ui: &mut Ui, text: &str, width: f32) -> Response {
    let b = egui::Button::new(RichText::new(text).color(Color32::BLACK).family(bold()).size(14.0))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .rounding(egui::Rounding::same(R))
        .min_size(Vec2::new(width, 38.0));
    ui.add_sized(Vec2::new(width, 38.0), b).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Subtle dark button.
pub fn button(ui: &mut Ui, text: &str) -> Response {
    let b = egui::Button::new(RichText::new(text).color(TEXT).size(13.0))
        .fill(CONTROL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .rounding(egui::Rounding::same(6.0))
        .min_size(Vec2::new(0.0, 30.0));
    ui.add(b).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A settings row: title + description on the left, control on the right.
/// Uses fixed rectangles so a wide control can never stretch the page.
pub fn setting_row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
    let split = (width * 0.42).clamp(180.0, 320.0);
    let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + split, rect.bottom()));
    let right = egui::Rect::from_min_max(egui::pos2(rect.left() + split, rect.top()), rect.max);

    let mut l = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::top_down(egui::Align::Min)));
    l.add_space(if desc.is_empty() { 16.0 } else { 7.0 });
    l.label(RichText::new(title).size(14.0).color(TEXT));
    if !desc.is_empty() {
        l.add(egui::Label::new(RichText::new(desc).size(12.0).color(TEXT_FAINT)).truncate());
    }
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
    control(&mut r);
}

/// Card container used for setting groups and clip rows.
pub fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .rounding(egui::Rounding::same(R + 2.0))
        .inner_margin(egui::Margin::symmetric(18.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

pub fn section_title(ui: &mut Ui, s: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(s.to_uppercase()).size(11.5).color(TEXT_FAINT).family(bold()));
    ui.add_space(2.0);
}

pub fn divider(ui: &mut Ui) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, BORDER);
}
