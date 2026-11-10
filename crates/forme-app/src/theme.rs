//! Visual identity: fonts, the palettes of each look, egui visuals.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// The colours of one look. The interfaces read them through `pal()`, so
/// each look (Atelier / Studio, Plaque, Radial) has its own palette.
#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_border: Color32,
    pub raised: Color32,
    pub well: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub text2: Color32,
    pub text_icon: Color32,
    pub label: Color32,
    /// Accent, also the Add colour.
    pub accent: Color32,
    pub accent_light: Color32,
    pub accent_bg: Color32,
    pub accent_border: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub carve: Color32,
    pub carve_light: Color32,
    pub neutral_mode: Color32,
    /// Tint of the selected shape in the 3D view (linear-ish 0..1).
    pub sel_rgb: [f32; 3],
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Graphite panels, blue = Add, orange = Carve (Atelier and Studio).
pub const PAL_CLASSIC: Pal = Pal {
    bg: c(0x17, 0x18, 0x1B),
    panel: c(0x20, 0x22, 0x26),
    panel_border: c(0x2F, 0x32, 0x37),
    raised: c(0x2A, 0x2D, 0x33),
    well: c(0x17, 0x18, 0x1B),
    divider: c(0x34, 0x37, 0x3D),
    text: c(0xEC, 0xED, 0xEF),
    text_strong: c(0xF2, 0xF3, 0xF5),
    text2: c(0xA3, 0xA7, 0xAE),
    text_icon: c(0xB4, 0xB8, 0xBF),
    label: c(0xC9, 0xCC, 0xD1),
    accent: c(0x5A, 0xA9, 0xFF),
    accent_light: c(0x9C, 0xCB, 0xFF),
    accent_bg: c(0x22, 0x34, 0x4B),
    accent_border: c(0x36, 0x55, 0x7A),
    accent_text: c(0xDC, 0xEB, 0xFF),
    on_accent: c(0x0B, 0x14, 0x22),
    carve: c(0xFF, 0x9A, 0x4D),
    carve_light: c(0xFF, 0xB2, 0x7A),
    neutral_mode: c(0xC8, 0xCC, 0xD2),
    sel_rgb: [0.24, 0.55, 1.0],
};

/// Plaque: cool neutral greys, one warm orange accent (= Add), steel blue Carve.
pub const PAL_PLAQUE: Pal = Pal {
    bg: c(0x16, 0x17, 0x19),
    panel: c(0x1D, 0x1E, 0x21),
    panel_border: c(0x2A, 0x2C, 0x30),
    raised: c(0x28, 0x2A, 0x2E),
    well: c(0x14, 0x15, 0x17),
    divider: c(0x2F, 0x31, 0x36),
    text: c(0xE6, 0xE7, 0xE9),
    text_strong: c(0xF5, 0xF5, 0xF6),
    text2: c(0x95, 0x99, 0xA0),
    text_icon: c(0xB1, 0xB5, 0xBB),
    label: c(0xC4, 0xC7, 0xCC),
    accent: c(0xEC, 0x8A, 0x3C),
    accent_light: c(0xF5, 0xAA, 0x6E),
    accent_bg: c(0x38, 0x26, 0x17),
    accent_border: c(0x6E, 0x44, 0x22),
    accent_text: c(0xFF, 0xE2, 0xC8),
    on_accent: c(0x1E, 0x11, 0x05),
    carve: c(0x6C, 0xA6, 0xE8),
    carve_light: c(0xA2, 0xC8, 0xF2),
    neutral_mode: c(0xC4, 0xC7, 0xCC),
    sel_rgb: [1.0, 0.48, 0.14],
};

/// Radial: warm near-black, acid green accent (= Add), coral Carve.
pub const PAL_RADIAL: Pal = Pal {
    bg: c(0x12, 0x13, 0x11),
    panel: c(0x1A, 0x1B, 0x19),
    panel_border: c(0x2D, 0x30, 0x2B),
    raised: c(0x26, 0x28, 0x24),
    well: c(0x10, 0x11, 0x0F),
    divider: c(0x31, 0x34, 0x2E),
    text: c(0xE3, 0xE6, 0xDE),
    text_strong: c(0xF2, 0xF5, 0xEC),
    text2: c(0x92, 0x97, 0x8D),
    text_icon: c(0xAE, 0xB3, 0xA8),
    label: c(0xC2, 0xC7, 0xBB),
    accent: c(0xA8, 0xF0, 0x5E),
    accent_light: c(0xC8, 0xF7, 0x95),
    accent_bg: c(0x25, 0x32, 0x19),
    accent_border: c(0x4B, 0x6B, 0x2D),
    accent_text: c(0xE4, 0xFA, 0xCD),
    on_accent: c(0x12, 0x1A, 0x09),
    carve: c(0xFF, 0x86, 0x5E),
    carve_light: c(0xFF, 0xAE, 0x93),
    neutral_mode: c(0xC2, 0xC7, 0xBB),
    sel_rgb: [0.5, 0.95, 0.22],
};

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Picks the palette used from now on (0 classic, 1 Plaque, 2 Radial).
pub fn set_palette(i: u8) {
    CURRENT.store(i, Ordering::Relaxed);
}

/// Index of the current palette.
pub fn palette_index() -> u8 {
    CURRENT.load(Ordering::Relaxed)
}

/// The current palette.
pub fn pal() -> &'static Pal {
    match CURRENT.load(Ordering::Relaxed) {
        1 => &PAL_PLAQUE,
        2 => &PAL_RADIAL,
        _ => &PAL_CLASSIC,
    }
}

pub const AXIS_X: Color32 = Color32::from_rgb(0xE0, 0x7A, 0x5F);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x8F, 0xC2, 0x7A);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x6F, 0xA8, 0xF5);
/// Selected edge (fillet tool).
pub const EDGE_SEL: Color32 = Color32::from_rgb(0xFF, 0xCF, 0x5A);
pub const EDGE_SEL_DIM: Color32 = Color32::from_rgb(0x6B, 0x55, 0x22);
pub const OK: Color32 = Color32::from_rgb(0x8F, 0xC2, 0x7A);
/// A bake that could not be made, a hard error.
pub const ERROR: Color32 = Color32::from_rgb(0xF2, 0x5F, 0x5C);

pub fn tint(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
/// JetBrains Mono Medium: the labels of the Radial look.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono".into()))
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
    fonts
        .font_data
        .insert("jbmono-medium".into(), Arc::new(FontData::from_static(include_bytes!("../assets/JetBrainsMono-Medium.ttf"))));
    let mut mono = vec!["jbmono-medium".to_string()];
    mono.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("mono".into()), mono);
    ctx.set_fonts(fonts);
    apply_visuals(ctx);
}

/// egui visuals from the current palette (again whenever the look changes).
pub fn apply_visuals(ctx: &egui::Context) {
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
        v.panel_fill = pal().panel;
        v.window_fill = pal().panel;
        v.extreme_bg_color = pal().well;
        v.faint_bg_color = pal().raised;
        v.override_text_color = Some(pal().text);
        v.selection.bg_fill = pal().accent_bg;
        v.selection.stroke = Stroke::new(1.5, pal().accent);
        v.window_corner_radius = CornerRadius::same(16);
        v.window_stroke = Stroke::new(1.0, pal().panel_border);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(10);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal().panel_border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal().text);
        v.widgets.inactive.bg_fill = pal().raised;
        v.widgets.inactive.weak_bg_fill = pal().raised;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.fg_stroke = Stroke::new(1.5, pal().text);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x37, 0x3E);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x33, 0x37, 0x3E);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal().accent_border);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, pal().text_strong);
        v.widgets.active.bg_fill = pal().accent_bg;
        v.widgets.active.weak_bg_fill = pal().accent_bg;
        v.widgets.active.bg_stroke = Stroke::new(1.0, pal().accent);
        v.widgets.active.fg_stroke = Stroke::new(1.5, pal().accent_text);
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
