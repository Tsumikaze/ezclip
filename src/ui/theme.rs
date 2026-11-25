//! Visual style: black, quiet, high-contrast text, white for "selected".
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Rounding, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(0, 0, 0);
pub const SURFACE: Color32 = Color32::from_rgb(12, 12, 12);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(20, 20, 20);
pub const BORDER: Color32 = Color32::from_rgb(30, 30, 30);
pub const CONTROL: Color32 = Color32::from_rgb(24, 24, 24);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(36, 36, 36);
pub const TEXT: Color32 = Color32::from_rgb(240, 240, 240);
pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 140, 140);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(90, 90, 90);
pub const ACCENT: Color32 = Color32::from_rgb(255, 255, 255);
pub const RED: Color32 = Color32::from_rgb(255, 69, 80);
pub const GREEN: Color32 = Color32::from_rgb(61, 220, 132);
pub const AMBER: Color32 = Color32::from_rgb(255, 184, 64);

pub const R: f32 = 8.0;

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn apply(ctx: &egui::Context) {
    // Use Windows' own UI font when available for a native, crisp look.
    let mut fonts = FontDefinitions::default();
    let regular = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf").ok();
    let semibold = std::fs::read("C:\\Windows\\Fonts\\seguisb.ttf").ok();
    let mut bold_list = fonts.families[&FontFamily::Proportional].clone();
    if let Some(r) = regular {
        fonts.font_data.insert("segoe".into(), FontData::from_owned(r));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "segoe".into());
        bold_list.insert(0, "segoe".into());
    }
    if let Some(b) = semibold {
        fonts.font_data.insert("segoe_sb".into(), FontData::from_owned(b));
        bold_list.insert(0, "segoe_sb".into());
    }
    fonts.families.insert(bold(), bold_list);
    ctx.set_fonts(fonts);

    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.extreme_bg_color = Color32::from_rgb(8, 8, 8);
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(70, 70, 70);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = TEXT;
    v.window_rounding = Rounding::same(R);
    v.menu_rounding = Rounding::same(R);
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_shadow = egui::epaint::Shadow::NONE;
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, SURFACE, BORDER),
        (&mut v.widgets.inactive, CONTROL, BORDER),
        (&mut v.widgets.hovered, CONTROL_HOVER, Color32::from_rgb(60, 60, 60)),
        (&mut v.widgets.active, Color32::from_rgb(48, 48, 48), Color32::from_rgb(80, 80, 80)),
        (&mut v.widgets.open, CONTROL_HOVER, Color32::from_rgb(60, 60, 60)),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0_f32, stroke);
        w.rounding = Rounding::same(6.0);
        w.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        use egui::{FontId, TextStyle};
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Heading, FontId::new(24.0, bold()));
    });
}
