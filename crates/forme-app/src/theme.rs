//! Visual identity from the mock-up: graphite panels, Inter, blue = Add, orange = Carve.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x1B);
pub const PANEL: Color32 = Color32::from_rgb(0x20, 0x22, 0x26);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x2F, 0x32, 0x37);
pub const RAISED: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x33);
pub const WELL: Color32 = Color32::from_rgb(0x17, 0x18, 0x1B);
pub const DIVIDER: Color32 = Color32::from_rgb(0x34, 0x37, 0x3D);
pub const TEXT: Color32 = Color32::from_rgb(0xEC, 0xED, 0xEF);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF2, 0xF3, 0xF5);
pub const TEXT2: Color32 = Color32::from_rgb(0xA3, 0xA7, 0xAE);
pub const TEXT_ICON: Color32 = Color32::from_rgb(0xB4, 0xB8, 0xBF);
pub const LABEL: Color32 = Color32::from_rgb(0xC9, 0xCC, 0xD1);
pub const ACCENT: Color32 = Color32::from_rgb(0x5A, 0xA9, 0xFF);
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x9C, 0xCB, 0xFF);
pub const ACCENT_BG: Color32 = Color32::from_rgb(0x22, 0x34, 0x4B);
pub const ACCENT_BORDER: Color32 = Color32::from_rgb(0x36, 0x55, 0x7A);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xDC, 0xEB, 0xFF);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x0B, 0x14, 0x22);
pub const CARVE: Color32 = Color32::from_rgb(0xFF, 0x9A, 0x4D);
pub const CARVE_LIGHT: Color32 = Color32::from_rgb(0xFF, 0xB2, 0x7A);
pub const NEUTRAL_MODE: Color32 = Color32::from_rgb(0xC8, 0xCC, 0xD2);
pub const AXIS_X: Color32 = Color32::from_rgb(0xE0, 0x7A, 0x5F);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x8F, 0xC2, 0x7A);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x6F, 0xA8, 0xF5);
/// Selected edge (fillet tool).
pub const EDGE_SEL: Color32 = Color32::from_rgb(0xFF, 0xCF, 0x5A);
pub const EDGE_SEL_DIM: Color32 = Color32::from_rgb(0x6B, 0x55, 0x22);
pub const OK: Color32 = Color32::from_rgb(0x8F, 0xC2, 0x7A);

pub fn tint(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf"))));
    fonts
        .font_data
        .insert("inter-semibold".into(), Arc::new(FontData::from_static(include_bytes!("../assets/Inter-SemiBold.ttf"))));
    fonts
        .font_data
        .insert("inter-medium".into(), Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Medium.ttf"))));
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["inter".to_string()];
    prop.extend(fallback.clone());
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut sb = vec!["inter-semibold".to_string()];
    sb.extend(fallback.clone());
    fonts.families.insert(FontFamily::Name("semibold".into()), sb);
    let mut md = vec!["inter-medium".to_string()];
    md.extend(fallback);
    fonts.families.insert(FontFamily::Name("medium".into()), md);
    ctx.set_fonts(fonts);

    ctx.global_style_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, regular(13.0));
        style.text_styles.insert(egui::TextStyle::Button, medium(13.0));
        style.text_styles.insert(egui::TextStyle::Small, regular(11.5));
        style.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.slider_width = 120.0;
        style.spacing.interact_size = egui::vec2(36.0, 28.0);
        style.interaction.tooltip_delay = 0.6;
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = WELL;
        v.faint_bg_color = RAISED;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT_BG;
        v.selection.stroke = Stroke::new(1.5, ACCENT);
        v.window_corner_radius = CornerRadius::same(16);
        v.window_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(10);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = RAISED;
        v.widgets.inactive.weak_bg_fill = RAISED;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.fg_stroke = Stroke::new(1.5, TEXT);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x37, 0x3E);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x33, 0x37, 0x3E);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_BORDER);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
        v.widgets.active.bg_fill = ACCENT_BG;
        v.widgets.active.weak_bg_fill = ACCENT_BG;
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.fg_stroke = Stroke::new(1.5, ACCENT_TEXT);
    });
}
