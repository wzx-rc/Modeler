//! Visual identity of the redesign: graphite panels, one accent per interface
//! (green for Établi, orange for Studio), yellow for what the pointer grabs.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x14);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x19, 0x1D);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x35);
pub const RAISED: Color32 = Color32::from_rgb(0x25, 0x29, 0x30);
pub const WELL: Color32 = Color32::from_rgb(0x0F, 0x11, 0x14);
pub const DIVIDER: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x35);
pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xE7, 0xEB);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF2, 0xF3, 0xF5);
pub const TEXT2: Color32 = Color32::from_rgb(0x9A, 0xA1, 0xAB);
pub const TEXT_ICON: Color32 = Color32::from_rgb(0xC3, 0xC8, 0xCF);
pub const LABEL: Color32 = Color32::from_rgb(0xB8, 0xBE, 0xC6);
pub const NEUTRAL_MODE: Color32 = Color32::from_rgb(0xC8, 0xCC, 0xD2);
pub const AXIS_X: Color32 = Color32::from_rgb(0xF0, 0x60, 0x5A);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x7C, 0xCB, 0x5B);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x4D, 0x8D, 0xFF);
/// Selected edge (fillet tool).
pub const EDGE_SEL: Color32 = Color32::from_rgb(0xFF, 0xCF, 0x5A);
pub const EDGE_SEL_DIM: Color32 = Color32::from_rgb(0x6B, 0x55, 0x22);
pub const OK: Color32 = Color32::from_rgb(0x8F, 0xC2, 0x7A);
/// A bake that could not be made, a hard error.
pub const ERROR: Color32 = Color32::from_rgb(0xF2, 0x5F, 0x5C);

/// Handle under the pointer or being dragged (WZX Engine's hot yellow).
pub const HOT: Color32 = Color32::from_rgb(0xFF, 0xE2, 0x66);

pub fn tint(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
/// Technical capitals (Établi's labels).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
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
    fonts.families.insert(FontFamily::Name("medium".into()), md.clone());
    // no monospace font is bundled: the « technical capitals » use Inter Medium
    fonts.families.insert(FontFamily::Monospace, md);
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
        v.selection.bg_fill = accent_bg();
        v.selection.stroke = Stroke::new(1.5, accent());
        v.window_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(7);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = RAISED;
        v.widgets.inactive.weak_bg_fill = RAISED;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.fg_stroke = Stroke::new(1.5, TEXT);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2C, 0x31, 0x39);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2C, 0x31, 0x39);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_border());
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
        v.widgets.active.bg_fill = accent_bg();
        v.widgets.active.weak_bg_fill = accent_bg();
        v.widgets.active.bg_stroke = Stroke::new(1.0, accent());
        v.widgets.active.fg_stroke = Stroke::new(1.5, accent_text());
    });
}

/// Bake diagnostic colours (linear-ish sRGB 0..1), by face role: fillet
/// strips orange, fill paths cream.
pub fn bake_role_color(patch: u32) -> [f32; 3] {
    use forme_core::bake::{P_FILL, P_FILLET};
    match patch {
        P_FILLET => [0.95, 0.52, 0.16],
        P_FILL => [0.93, 0.86, 0.68],
        _ => [0.80, 0.30, 0.55],
    }
}

/// Construction lines in the Bake panel: sharp lines and fillet rails.
pub const LINE_SHARP: Color32 = Color32::from_rgb(0xF5, 0xD0, 0x45);
pub const LINE_RAIL: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x2E);

// ------------------------------------------------------------ ambiances
//
// Each interface has its own accent (Romain's two moods, 10 nov.):
// Établi = « menu radial, accent vert » (Add green, Carve orange),
// Studio = « sobre, façon Plasticity », accent orange (Add orange, Carve blue).
// The accent marks Add, the selection (also its tint in the 3D view) and the
// active tool. `set_ambiance` is called once per frame by the interface.

static AMBIANCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ambiance {
    /// Établi: green accent.
    Vert,
    /// Studio: orange accent.
    Orange,
}

pub fn ambiance() -> Ambiance {
    if AMBIANCE.load(std::sync::atomic::Ordering::Relaxed) == 1 {
        Ambiance::Orange
    } else {
        Ambiance::Vert
    }
}

/// Sets the ambiance and the egui colours that depend on it.
pub fn set_ambiance(ctx: &egui::Context, a: Ambiance) {
    let code = if a == Ambiance::Orange { 1 } else { 0 };
    let before = AMBIANCE.swap(code, std::sync::atomic::Ordering::Relaxed);
    let key = egui::Id::new("ambiance-styled");
    let styled: Option<u8> = ctx.data(|d| d.get_temp(key));
    if before != code || styled != Some(code) {
        ctx.data_mut(|d| d.insert_temp(key, code));
        ctx.global_style_mut(|style| {
            let v = &mut style.visuals;
            v.selection.bg_fill = accent_bg();
            v.selection.stroke = Stroke::new(1.5, accent());
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_border());
            v.widgets.active.bg_fill = accent_bg();
            v.widgets.active.weak_bg_fill = accent_bg();
            v.widgets.active.bg_stroke = Stroke::new(1.0, accent());
            v.widgets.active.fg_stroke = Stroke::new(1.5, accent_text());
        });
    }
}

fn by(vert: Color32, orange: Color32) -> Color32 {
    if ambiance() == Ambiance::Orange {
        orange
    } else {
        vert
    }
}

pub fn accent() -> Color32 {
    by(Color32::from_rgb(0x8F, 0xDB, 0x5B), Color32::from_rgb(0xF2, 0x8C, 0x38))
}
pub fn accent_light() -> Color32 {
    by(Color32::from_rgb(0xB6, 0xEC, 0x92), Color32::from_rgb(0xFF, 0xB8, 0x7A))
}
pub fn accent_bg() -> Color32 {
    by(Color32::from_rgb(0x1C, 0x2B, 0x17), Color32::from_rgb(0x34, 0x22, 0x14))
}
pub fn accent_border() -> Color32 {
    by(Color32::from_rgb(0x3A, 0x5C, 0x2B), Color32::from_rgb(0x74, 0x43, 0x1E))
}
pub fn accent_text() -> Color32 {
    by(Color32::from_rgb(0xE2, 0xF7, 0xD5), Color32::from_rgb(0xFF, 0xE4, 0xCC))
}
/// Text and icons drawn on a filled accent.
pub fn on_accent() -> Color32 {
    by(Color32::from_rgb(0x10, 0x1C, 0x0A), Color32::from_rgb(0x22, 0x12, 0x06))
}
pub fn carve() -> Color32 {
    by(Color32::from_rgb(0xF0, 0x8A, 0x3C), Color32::from_rgb(0x5B, 0x9B, 0xFF))
}
pub fn carve_light() -> Color32 {
    by(Color32::from_rgb(0xFF, 0xB2, 0x7A), Color32::from_rgb(0x9C, 0xC3, 0xFF))
}
/// Selection tint of the 3D view (0..1).
pub fn sel_rgb() -> [f32; 3] {
    let c = accent();
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

// ---------------------------------------------------------------- app icon

/// The app icon (Recraft: a white drilled « F » on a graphite tile), 256 px.
const ICON_PNG: &[u8] = include_bytes!("../assets/forme-icon.png");
/// The same, small, for the logo of the interfaces.
const LOGO_PNG: &[u8] = include_bytes!("../assets/forme-logo-64.png");

fn decode(png: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

/// Window / taskbar icon.
pub fn window_icon() -> Option<winit::window::Icon> {
    let (rgba, w, h) = decode(ICON_PNG)?;
    winit::window::Icon::from_rgba(rgba, w, h).ok()
}

/// Draws the app logo in `rect` (texture loaded once).
pub fn paint_logo(ui: &egui::Ui, rect: egui::Rect) {
    let key = egui::Id::new("forme-logo-tex");
    let tex: Option<egui::TextureHandle> = ui.ctx().data(|d| d.get_temp(key));
    let tex = match tex {
        Some(t) => t,
        None => {
            let Some((rgba, w, h)) = decode(LOGO_PNG) else { return };
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            let t = ui.ctx().load_texture("forme-logo", img, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(key, t.clone()));
            t
        }
    };
    ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
}