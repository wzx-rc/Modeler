//! The Studio interface (Romain 22 nov.: "implémenter le D3 mais en thème
//! dans setting qui change totalement l'UI et UX"), the mock-up D3 « Studio
//! doux »: soft greys, one soft accent (blue in Crisp, the Forme tab; green
//! in Soft, the SDF tab), translucent white or graphite panels.
//!
//! - top left the document (the files, its name, undo, redo); top centre
//!   the tabs, named Crisp and Soft; top right the view (Perspective ▾),
//!   See through and the settings;
//! - the scene on the left: the elements, an element's operations or
//!   shapes under it;
//! - an inspector on the right for what is selected or picked: big − / +
//!   steps, the material balls, the position folded;
//! - the tools in a dock at the bottom, simple words (Block, Tube, Round,
//!   Push, Cut; Ball, Bean, Ring), Build / Dig (Add / Carve / Overlap in
//!   Soft) in it;
//! - a pill of actions over the selection (Copy, Mirror, Repeat, Hollow,
//!   Delete), the sizes on the part as chips (a tap types them), the view
//!   cube and Frame at the bottom right, a hint at the top of the view.
//!
//! The same app and tools; the iPad's large rows and keypad (`tablet()`),
//! on the PC too. Left-handed swaps the scene and the inspector.

use super::tablet::{flag, pressed_outside, set_flag};
use super::*;
use crate::touch::{BAR_EDGE, KEY_D, KEY_GAP};
use crate::widgets::Keypad;

/// The margin to the screen's edges, the top islands' height, where the
/// panels begin, their widths, the dock's height, the view cube's size.
const E: f32 = 16.0;
const BAR_H: f32 = 52.0;
const TOP: f32 = E + BAR_H + 16.0;
const SCENE_W: f32 = 248.0;
const INSP_W: f32 = 300.0;
const DOCK_H: f32 = 92.0;
const CUBE: f32 = 64.0;
const FRAME_H: f32 = 48.0;
/// The navigation grips over the view cube (Zoom, Pan: a Pencil's target)
/// and their pill's height.
const GRIP: f32 = 48.0;
const NAV_H: f32 = 2.0 * GRIP + 4.0 + 8.0;

fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// A segmented row's well, its lit button, the − / + buttons, a value's
/// field and its line, the quiet text.
fn seg_bg() -> Color32 {
    pick(rgb(0x2A, 0x2E, 0x34), rgb(0xF1, 0xF3, 0xF5))
}
fn seg_on() -> Color32 {
    pick(rgb(0x3A, 0x3F, 0x47), Color32::WHITE)
}
fn btn_bg() -> Color32 {
    pick(rgb(0x2E, 0x32, 0x38), rgb(0xF1, 0xF3, 0xF5))
}
fn btn_down() -> Color32 {
    pick(rgb(0x3A, 0x3F, 0x47), rgb(0xE2, 0xE5, 0xEA))
}
fn field_bg() -> Color32 {
    pick(rgb(0x1F, 0x22, 0x27), Color32::WHITE)
}
fn field_line() -> Color32 {
    pick(Color32::from_rgba_unmultiplied(255, 255, 255, 26), Color32::from_rgba_unmultiplied(20, 28, 40, 26))
}
fn muted() -> Color32 {
    pick(rgb(0x8E, 0x95, 0x9F), rgb(0x6B, 0x73, 0x7E))
}
fn ink() -> Color32 {
    pick(rgb(0xC9, 0xCD, 0xD3), rgb(0x3A, 0x40, 0x4A))
}

fn shadow(big: bool) -> egui::Shadow {
    let a = if big { 102 } else { 89 };
    egui::Shadow { offset: [0, if big { 12 } else { 8 }], blur: if big { 32 } else { 28 }, spread: 0, color: pick(Color32::from_black_alpha(a), Color32::from_rgba_unmultiplied(20, 28, 40, if big { 26 } else { 20 })) }
}

/// A filled rounded rectangle with its shadow under it.
fn shadowed(p: &egui::Painter, r: Rect, radius: f32, fill: Color32, sh: egui::Shadow) {
    p.add(sh.as_shape(r, egui::CornerRadius::same(radius as u8)));
    p.rect_filled(r, radius, fill);
}

/// The mock-up's panel: translucent, a hairline, rounded, a soft shadow.
fn glass(radius: u8) -> egui::Frame {
    egui::Frame::new().fill(tint(PANEL(), 0.88)).stroke(Stroke::new(1.0, PANEL_BORDER())).corner_radius(egui::CornerRadius::same(radius)).shadow(shadow(false))
}

/// A pop-over (a fly-out, the pill): nearly opaque.
fn pop_glass(radius: u8) -> egui::Frame {
    egui::Frame::new()
        .fill(pick(Color32::from_rgba_unmultiplied(44, 48, 55, 240), Color32::from_rgba_unmultiplied(255, 255, 255, 235)))
        .stroke(Stroke::new(1.0, pick(Color32::from_rgba_unmultiplied(255, 255, 255, 20), Color32::from_rgba_unmultiplied(20, 28, 40, 16))))
        .corner_radius(egui::CornerRadius::same(radius))
        .shadow(shadow(true))
}

pub fn show(ctx: &egui::Context, st: &mut AppState) {
    let screen = ctx.content_rect();
    let flip = st.settings.left_handed;
    let side = st.panel_visible;
    // the view between the scene and the inspector, above the dock; the
    // inspector's room kept when it is not shown, so the part stays put
    let scene_w = if side { E + SCENE_W + 8.0 } else { E };
    let insp_w = E + INSP_W + 8.0;
    let (l, r) = if flip { (insp_w, screen.width() - scene_w) } else { (scene_w, screen.width() - insp_w) };
    st.view_rect = [screen.min.x + l, TOP, screen.min.x + r, screen.max.y - E - DOCK_H - 8.0];
    doc_island(ctx, st, screen);
    tabs_island(ctx, st, screen);
    view_island(ctx, st, screen);
    if side {
        scene_panel(ctx, st, screen, flip);
    } else {
        scene_button(ctx, st, screen, flip);
    }
    inspector(ctx, st, screen, flip);
    dock(ctx, st, screen);
    view_cube(ctx, st, screen, flip);
    hint_pill(ctx, st);
    size_chips(ctx, st);
    actions_pill(ctx, st);
    group_bar(ctx, st);
    align_window(ctx, st);
    deviation_legend(ctx, st);
    shapes_fly(ctx, st);
    more_fly(ctx, st);
    paint_pop(ctx, st);
    super::tablet::view_menu(ctx, st);
    super::tablet::settings_pop(ctx, st);
    super::tablet::gesture_sheet(ctx, st, screen);
    files_dialog(ctx, st, screen);
    toast(ctx, st, screen);
    keypad(ctx);
    shortcuts(ctx, st);
}

// ------------------------------------------------------------ small widgets

/// An icon button of the top islands (44 points), lit with the accent.
fn top_icon(ui: &mut Ui, icon: Icon, on: bool, tip: &str, enabled: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    if on {
        p.rect_filled(r, 12.0, accent_bg());
    } else if enabled && (resp.is_pointer_button_down_on() || resp.hovered()) {
        p.rect_filled(r, 12.0, RAISED());
    }
    let c = if on { accent_text() } else if enabled { ink() } else { tint(ink(), 0.35) };
    icons::paint(p, Rect::from_center_size(r.center(), vec2(20.0, 20.0)), icon, c);
    resp.on_hover_text(tip)
}

fn bar_sep(ui: &mut Ui, h: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, h), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(1.0, (h * 0.55).max(20.0))), 0.0, DIVIDER());
}

/// A segmented row the inspector's width: a quiet well, the current one
/// raised (the mock-up's Add / Carve / Overlap).
fn seg<T: PartialEq + Copy>(ui: &mut Ui, id: &str, items: &[(T, &str)], current: T, h: f32) -> Option<T> {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().rect_filled(rect, 12.0, seg_bg());
    let n = items.len() as f32;
    let inner = rect.shrink(4.0);
    let bw = (inner.width() - 4.0 * (n - 1.0)) / n;
    let mut picked = None;
    for (i, (v, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(inner.min + vec2(i as f32 * (bw + 4.0), 0.0), vec2(bw, inner.height()));
        let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
        let on = *v == current;
        let p = ui.painter();
        if on {
            p.rect_filled(r.translate(vec2(0.0, 1.0)), 9.0, Color32::from_black_alpha(if light() { 18 } else { 60 }));
            p.rect_filled(r, 9.0, seg_on());
        } else if resp.is_pointer_button_down_on() {
            p.rect_filled(r, 9.0, btn_down());
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, if on { semibold(13.0) } else { medium(13.0) }, if on { TEXT_STRONG() } else { TEXT2() });
        if resp.clicked() && !on {
            picked = Some(*v);
        }
    }
    picked
}

/// A row of pills the inspector's width (Round / Chamfer, Mirror X Y Z):
/// those on in the accent; the one tapped.
fn pills<T: Copy>(ui: &mut Ui, id: &str, items: &[(T, &str, bool)], h: f32) -> Option<T> {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let n = items.len() as f32;
    let bw = (w - 6.0 * (n - 1.0)) / n;
    let mut picked = None;
    for (i, (v, label, on)) in items.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(i as f32 * (bw + 6.0), 0.0), vec2(bw, h));
        let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
        let p = ui.painter();
        if *on {
            p.rect(r, 10.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
        } else {
            p.rect_filled(r, 10.0, if resp.is_pointer_button_down_on() { btn_down() } else { seg_bg() });
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, if *on { semibold(12.5) } else { medium(12.5) }, if *on { accent_text() } else { TEXT2() });
        if resp.clicked() {
            picked = Some(*v);
        }
    }
    picked
}

/// A group's small title in the inspector ("Size", "Material").
fn label(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(egui::RichText::new(text).font(semibold(12.0)).color(TEXT2()));
    ui.add_space(4.0);
}

/// The inspector's title: the name, a chip for its kind, a note on the right.
fn header(ui: &mut Ui, name: &str, chip: &str, lit: bool, note: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let w = ui.available_width();
        let g_note = ui.painter().layout_no_wrap(note.to_string(), regular(12.0), muted());
        let g_chip = ui.painter().layout_no_wrap(chip.to_string(), semibold(12.0), if lit { accent_text() } else { TEXT2() });
        let chip_w = if chip.is_empty() { 0.0 } else { g_chip.size().x + 16.0 + 10.0 };
        let max_name = (w - chip_w - g_note.size().x - 12.0).max(60.0);
        let mut job = egui::text::LayoutJob::single_section(name.to_string(), egui::TextFormat::simple(semibold(17.0), TEXT_STRONG()));
        job.wrap = egui::text::TextWrapping { max_width: max_name, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x, 26.0), Sense::hover());
        ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, TEXT_STRONG());
        if !chip.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(g_chip.size().x + 16.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r, 7.0, if lit { accent_bg() } else { seg_bg() });
            ui.painter().galley(r.center() - g_chip.size() * 0.5, g_chip, TEXT2());
        }
        if !note.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(g_note.size().x, 22.0), Sense::hover());
                ui.painter().galley(pos2(r.min.x, r.center().y - g_note.size().y * 0.5), g_note, muted());
            });
        }
    });
}

/// A value with − and + (the mock-up's sizes): its label on the left,
/// the value in the middle (a tap: the keypad; a drag across: finer), one
/// step a tap. `hot`: ringed in the accent. (changed, a new step began).
fn stepper(ui: &mut Ui, name: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, suffix: &str, hot: bool) -> (bool, bool) {
    let (lo, hi) = (*range.start(), *range.end());
    let mut out = (false, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if !name.is_empty() {
            let (lr, _) = ui.allocate_exact_size(vec2(72.0, 40.0), Sense::hover());
            ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, name, regular(13.0), ink());
        }
        let inc = step_of(*v, step, suffix);
        let bump = |ui: &mut Ui, sign: f32, v: &mut f32| -> bool {
            let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(r, 11.0, if resp.is_pointer_button_down_on() { btn_down() } else { btn_bg() });
            icons::paint(p, Rect::from_center_size(r.center(), vec2(16.0, 16.0)), if sign < 0.0 { Icon::Minus } else { Icon::Plus }, ink());
            if resp.clicked() {
                // on the step's grid, then one step
                let base = (*v / inc).round() * inc;
                let nv = round3((if (base - *v).abs() > 1e-4 && (base - *v).signum() == sign { base } else { base + sign * inc }).clamp(lo, hi));
                if (nv - *v).abs() > 1e-7 {
                    *v = nv;
                    return true;
                }
            }
            false
        };
        if bump(ui, -1.0, v) {
            out = (true, true);
        }
        let fw = (ui.available_width() - 48.0).max(60.0);
        let (r, resp) = ui.allocate_exact_size(vec2(fw, 40.0), Sense::click_and_drag());
        // the keypad's answer, the frame after its Done
        if let Some(x) = ui.data_mut(|d| d.remove_temp::<f32>(resp.id.with("kp-result"))) {
            let nv = round3(x.clamp(lo, hi));
            if (nv - *v).abs() > 1e-7 {
                *v = nv;
                out = (true, true);
            }
        }
        if resp.clicked() {
            let kp = Keypad { owner: resp.id, text: fmt3(*v as f64), fresh: true, suffix: suffix.to_string(), lo, hi, anchor: r };
            ui.data_mut(|d| d.insert_temp(Id::new("keypad"), kp));
        }
        let raw_key = resp.id.with("raw");
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(raw_key, *v as f64));
            out.1 = true;
        }
        if resp.dragged() {
            let dx = ui.input(|i| i.pointer.delta().x);
            let raw = (ui.data(|d| d.get_temp::<f64>(raw_key)).unwrap_or(*v as f64) + dx as f64 * step).clamp(lo as f64, hi as f64);
            ui.data_mut(|d| d.insert_temp(raw_key, raw));
            let nv = round3(((raw / step).round() * step) as f32);
            if (nv - *v).abs() > 1e-7 {
                *v = nv;
                out.0 = true;
            }
        }
        let typing = ui.data(|d| d.get_temp::<Keypad>(Id::new("keypad"))).filter(|k| k.owner == resp.id);
        let p = ui.painter();
        let ring = if typing.is_some() { Stroke::new(1.6, TEXT_STRONG()) } else if hot { Stroke::new(1.5, accent()) } else { Stroke::new(1.0, field_line()) };
        p.rect(r, 11.0, field_bg(), ring, StrokeKind::Inside);
        let num = match &typing {
            Some(k) => k.text.clone(),
            None if step >= 1.0 && v.fract() == 0.0 => format!("{}", *v as i64),
            None => fmt3(*v as f64),
        };
        let mut job = egui::text::LayoutJob::default();
        job.append(&num, 0.0, egui::TextFormat::simple(semibold(14.0), TEXT_STRONG()));
        if !suffix.is_empty() {
            job.append(suffix, 4.0, egui::TextFormat::simple(medium(14.0), muted()));
        }
        let g = p.layout_job(job);
        p.galley(r.center() - g.size() * 0.5, g, TEXT_STRONG());
        if bump(ui, 1.0, v) {
            out = (true, true);
        }
    });
    *v = v.clamp(lo, hi);
    out
}

/// A switch row: its label, the switch on the right.
fn switch_row(ui: &mut Ui, name: &str, on: bool) -> bool {
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, name, regular(13.0), ink());
    let sr = Rect::from_center_size(pos2(r.max.x - 24.0, r.center().y), vec2(46.0, 28.0));
    let p = ui.painter();
    p.rect_filled(sr, 14.0, if on { accent() } else { seg_bg() });
    let k = if on { sr.right_center() - vec2(14.0, 0.0) } else { sr.left_center() + vec2(14.0, 0.0) };
    p.circle_filled(k + vec2(0.0, 1.0), 11.0, Color32::from_black_alpha(40));
    p.circle_filled(k, 11.0, if on { on_accent() } else { Color32::WHITE });
    resp.clicked()
}

/// A button the width given: quiet, or filled in the accent.
fn wide_btn(ui: &mut Ui, text: &str, w: f32, filled: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let p = ui.painter();
    if filled {
        p.rect_filled(r, 12.0, if resp.is_pointer_button_down_on() { accent_light() } else { accent() });
        p.text(r.center(), Align2::CENTER_CENTER, text, semibold(14.0), on_accent());
    } else {
        p.rect_filled(r, 12.0, if resp.is_pointer_button_down_on() { btn_down() } else { btn_bg() });
        p.text(r.center(), Align2::CENTER_CENTER, text, medium(14.0), TEXT());
    }
    resp
}

/// The material balls, the current one ringed, as big as the row lets them
/// be (at most 36 points); a tap gives it to what is selected or picked
/// (`apply_material`).
fn swatches(ui: &mut Ui, st: &mut AppState) {
    let cur = current_material(st).unwrap_or(st.material);
    let n = materials::MATERIALS.len() as f32;
    let gap = 6.0;
    let s = ((ui.available_width() - gap * (n - 1.0)) / n).clamp(24.0, 36.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for i in 0..materials::MATERIALS.len() as u32 {
            let (r, resp) = ui.allocate_exact_size(vec2(s, s), Sense::click());
            let p = ui.painter();
            if i == cur {
                p.circle_stroke(r.center(), s * 0.54, Stroke::new(2.0, accent()));
            }
            material_ball(p, r.center(), s * if i == cur { 0.42 } else { 0.47 }, &st.look(i));
            if resp.on_hover_text(materials::get(i).name).clicked() {
                st.apply_material(i);
            }
        }
    });
}

/// A row that folds what follows ("Position, turn and repeat ›").
fn fold_row(ui: &mut Ui, text: &str, key: &str) -> bool {
    let id = Id::new(("s-fold", key));
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    ui.add_space(10.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 10.0, btn_down());
    }
    p.text(r.left_center(), Align2::LEFT_CENTER, text, regular(13.0), ink());
    icons::paint(p, Rect::from_center_size(pos2(r.max.x - 10.0, r.center().y), vec2(14.0, 14.0)), if open { Icon::Chevron } else { Icon::ChevronRight }, muted());
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(id, !open));
    }
    open
}

// ------------------------------------------------------------ the top

/// Top left: the documents, the name, undo, redo.
fn doc_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("s-doc")).order(Order::Middle).fixed_pos(pos2(screen.min.x + E, screen.min.y + E)).show(ctx, |ui| {
        glass(16).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.set_height(BAR_H - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if top_icon(ui, Icon::Grid, st.files_open, "All documents", true).clicked() {
                    st.files_open = !st.files_open;
                }
                let n = st.doc.features.iter().filter(|f| f.is_solid()).count();
                let g = ui.painter().layout_no_wrap(st.doc_name.clone(), semibold(15.0), TEXT_STRONG());
                let w = g.size().x.min(220.0) + 18.0;
                let (r, resp) = ui.allocate_exact_size(vec2(w.max(90.0), 44.0), Sense::click());
                let p = ui.painter();
                if resp.is_pointer_button_down_on() {
                    p.rect_filled(r, 12.0, RAISED());
                }
                let mut job = egui::text::LayoutJob::single_section(st.doc_name.clone(), egui::TextFormat::simple(semibold(15.0), TEXT_STRONG()));
                job.wrap = egui::text::TextWrapping { max_width: 220.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = p.layout_job(job);
                p.galley(pos2(r.min.x + 6.0, r.center().y - g.size().y - 0.5), g, TEXT_STRONG());
                p.text(pos2(r.min.x + 6.0, r.center().y + 1.5), Align2::LEFT_TOP, format!("{n} shape{}", if n == 1 { "" } else { "s" }), medium(12.0), TEXT2());
                if resp.clicked() {
                    st.files_open = !st.files_open;
                }
                bar_sep(ui, 44.0);
                if top_icon(ui, Icon::Undo, false, "Undo", st.can_undo()).clicked() {
                    st.undo();
                }
                if top_icon(ui, Icon::Redo, false, "Redo", st.can_redo()).clicked() {
                    st.redo();
                }
            });
        });
    });
}

/// The mock-up's names for the tabs: Crisp (Forme), Soft (SDF).
fn tab_name(w: Workspace) -> &'static str {
    match w {
        Workspace::Forme => "Crisp",
        Workspace::Sdf => "Soft",
        _ => w.label(),
    }
}

/// Top centre: the tabs.
fn tabs_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    let tabs = st.workspaces();
    egui::Area::new(Id::new("s-tabs")).order(Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(pos2(screen.center().x, screen.min.y + E)).show(ctx, |ui| {
        glass(16).inner_margin(Margin::same(5)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for w in tabs {
                    let (r, resp) = ui.allocate_exact_size(vec2(113.0, BAR_H - 12.0), Sense::click());
                    let on = st.workspace == w;
                    let p = ui.painter();
                    if on {
                        p.rect(r, 11.0, accent_bg(), Stroke::new(1.0, accent_border()), StrokeKind::Inside);
                    } else if resp.is_pointer_button_down_on() {
                        p.rect_filled(r, 11.0, RAISED());
                    }
                    let c = if on { accent_text() } else { TEXT2() };
                    let name = tab_name(w);
                    let g = p.layout_no_wrap(name.to_string(), if on { semibold(14.0) } else { medium(14.0) }, c);
                    let x0 = r.center().x - (g.size().x + 24.0) * 0.5;
                    icons::paint(p, Rect::from_center_size(pos2(x0 + 8.0, r.center().y), vec2(16.0, 16.0)), w.icon(), c);
                    p.galley(pos2(x0 + 24.0, r.center().y - g.size().y * 0.5), g, c);
                    if resp.clicked() && !on {
                        st.set_workspace(w);
                    }
                }
            });
        });
    });
}

/// Top right: the view (Perspective ▾), See through, the settings.
fn view_island(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("s-view")).order(Order::Middle).pivot(Align2::RIGHT_TOP).fixed_pos(pos2(screen.max.x - E, screen.min.y + E)).show(ctx, |ui| {
        glass(16).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.set_height(BAR_H - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let name = match st.camera.preset_of() {
                    ViewPreset::Perspective => "Perspective",
                    ViewPreset::Front => "Front",
                    ViewPreset::Right => "Right",
                    ViewPreset::Top => "Top",
                };
                let g = ui.painter().layout_no_wrap(name.to_string(), medium(14.0), TEXT());
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 44.0, 44.0), Sense::click());
                let open = flag(ctx, "view");
                if open || resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(r, 12.0, RAISED());
                }
                ui.painter().galley(pos2(r.min.x + 12.0, r.center().y - g.size().y * 0.5), g, TEXT());
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.max.x - 16.0, r.center().y), vec2(13.0, 13.0)), Icon::Chevron, TEXT2());
                ctx.data_mut(|d| d.insert_temp(Id::new("t-view-btn"), r));
                if resp.clicked() {
                    set_flag(ctx, "view", !open);
                    set_flag(ctx, "settings", false);
                }
                if top_icon(ui, Icon::Xray, st.xray, "See through (X-ray)", true).clicked() {
                    st.xray = !st.xray;
                }
                let r = top_icon(ui, Icon::Sliders, flag(ctx, "settings"), "Settings", true);
                ctx.data_mut(|d| d.insert_temp(Id::new("t-settings-btn"), r.rect));
                if r.clicked() {
                    let open = flag(ctx, "settings");
                    set_flag(ctx, "settings", !open);
                    set_flag(ctx, "view", false);
                }
            });
        });
    });
}

// ------------------------------------------------------------ the scene

/// The scene folded: one button to bring it back.
fn scene_button(ctx: &egui::Context, st: &mut AppState, screen: Rect, flip: bool) {
    let x = if flip { screen.max.x - E - 52.0 } else { screen.min.x + E };
    egui::Area::new(Id::new("s-scene-btn")).order(Order::Middle).fixed_pos(pos2(x, TOP)).show(ctx, |ui| {
        glass(16).inner_margin(Margin::same(4)).show(ui, |ui| {
            if top_icon(ui, Icon::Sidebar, false, "Show the scene", true).clicked() {
                st.panel_visible = true;
            }
        });
    });
}

/// The bottom the side panels keep above: the iPad's Shift / Ctrl keys on
/// the scene's side, the view cube on the inspector's.
fn scene_bottom(st: &AppState, screen: Rect) -> f32 {
    if st.touch.on {
        screen.max.y - BAR_EDGE - 2.0 * KEY_D - KEY_GAP - 14.0
    } else {
        screen.max.y - E - DOCK_H - 16.0
    }
}

/// The scene (the mock-up's): each element a row (its arrow, icon, name,
/// how many operations or shapes), opened under it when it is the one
/// worked on or its arrow is tapped; groups with their shapes under them.
fn scene_panel(ctx: &egui::Context, st: &mut AppState, screen: Rect, flip: bool) {
    let x = if flip { screen.max.x - E - SCENE_W } else { screen.min.x + E };
    let max_h = (scene_bottom(st, screen) - TOP).max(160.0);
    egui::Area::new(Id::new("s-scene")).order(Order::Middle).fixed_pos(pos2(x, TOP)).show(ctx, |ui| {
        glass(20).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(SCENE_W - 16.0);
            ui.horizontal(|ui| {
                ui.set_height(40.0);
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Scene").font(semibold(13.0)).color(TEXT2()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
                    if resp.is_pointer_button_down_on() {
                        ui.painter().rect_filled(r, 10.0, RAISED());
                    }
                    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), Icon::Sidebar, ink());
                    if resp.on_hover_text("Hide the scene").clicked() {
                        st.panel_visible = false;
                    }
                });
            });
            egui::ScrollArea::vertical().id_salt("s-scene-scroll").max_height(max_h - 64.0).min_scrolled_height(max_h - 64.0).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                scene_rows(ui, st);
            });
        });
    });
}

fn scene_rows(ui: &mut Ui, st: &mut AppState) {
    use crate::common::{kind_of, Kind};
    let feats = st.doc.features.clone();
    if feats.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Nothing yet: pick a shape in the dock").font(regular(13.0)).color(TEXT2()));
        ui.add_space(6.0);
        return;
    }
    let dead = st.doc.orphans().len();
    if dead > 0 {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        let p = ui.painter();
        p.rect_filled(r, 10.0, if resp.is_pointer_button_down_on() { btn_down() } else { seg_bg() });
        p.text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, format!("{dead} leftover operation{}", if dead > 1 { "s" } else { "" }), regular(12.5), carve_light());
        p.text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, "Remove", semibold(12.5), TEXT());
        if resp.clicked() {
            st.remove_orphans();
        }
        ui.add_space(4.0);
    }
    if st.doc.reference.is_some() {
        reference_rows(ui, st);
    }
    let focus = st.outline_element();
    // the groups, their shapes under them
    for g in st.doc.groups.clone() {
        let id = Id::new(("s-open-group", g.id));
        let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
        let selected = st.selected_group().map(|x| x.id) == Some(g.id);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
        let arrow = Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(28.0, 40.0));
        let aresp = ui.interact(arrow, Id::new(("s-arrow-group", g.id)), Sense::click());
        row_look(ui, r, selected, resp.is_pointer_button_down_on());
        let p = ui.painter();
        icons::paint(p, Rect::from_center_size(arrow.center(), vec2(12.0, 12.0)), if open { Icon::Chevron } else { Icon::ChevronRight }, muted());
        icons::paint(p, Rect::from_center_size(pos2(r.min.x + 40.0, r.center().y), vec2(17.0, 17.0)), Icon::Group, if selected { accent_text() } else { ink() });
        row_text(ui, r, &g.name, &format!("{}", g.members.len()), selected, true, 1.0);
        if aresp.clicked() {
            ui.data_mut(|d| d.insert_temp(id, !open));
        } else if resp.clicked() {
            st.select_group(g.id, false);
        }
        if open {
            for m in g.members.clone() {
                if let Some(f) = st.doc.feature(m).cloned() {
                    element_row(ui, st, &f, 18.0, focus);
                }
            }
        }
    }
    // the elements, in the order they were made
    for f in feats.iter() {
        let el = kind_of(f).is_some_and(|k| k != Kind::Curve || st.doc.boolean_of(f.id).is_none()) && st.doc.group_of(f.id).is_none();
        if el {
            element_row(ui, st, f, 0.0, focus);
        }
    }
    // the operations whose element cannot be told
    let loose: Vec<&Feature> = feats.iter().filter(|f| !f.is_solid() && !f.is_path() && st.owner_of(f.id).is_none() && kind_of(f).is_none()).collect();
    if !loose.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Other operations").font(semibold(12.0)).color(TEXT2()));
        for f in loose {
            item_line(ui, st, crate::common::Item::Feature(f.id), 0.0);
        }
    }
    ui.add_space(4.0);
}

fn row_look(ui: &Ui, r: Rect, selected: bool, down: bool) {
    let p = ui.painter();
    if selected {
        p.rect_filled(r, 12.0, accent_bg());
    } else if down {
        p.rect_filled(r, 12.0, RAISED());
    }
}

/// A row's name and its count or note on the right.
fn row_text(ui: &Ui, r: Rect, name: &str, note: &str, selected: bool, strong: bool, dim: f32) {
    let p = ui.painter();
    let gn = p.layout_no_wrap(note.to_string(), regular(12.0), if selected { accent_text() } else { tint(muted(), dim) });
    let nx = r.max.x - 10.0 - gn.size().x;
    p.galley(pos2(nx, r.center().y - gn.size().y * 0.5), gn, muted());
    let c = if selected { accent_text() } else { tint(TEXT(), dim) };
    let mut job = egui::text::LayoutJob::single_section(name.to_string(), egui::TextFormat::simple(if strong { semibold(14.0) } else { medium(14.0) }, c));
    job.wrap = egui::text::TextWrapping { max_width: (nx - 8.0 - (r.min.x + 54.0)).max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = p.layout_job(job);
    p.galley(pos2(r.min.x + 54.0, r.center().y - g.size().y * 0.5), g, c);
}

/// An element's row (a shape, a curve, an SDF object), its items under it
/// when open.
fn element_row(ui: &mut Ui, st: &mut AppState, f: &Feature, indent: f32, focus: Option<FeatureId>) {
    let items = st.element_items(f.id);
    let id = Id::new(("s-open", f.id));
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(focus == Some(f.id)) && !items.is_empty();
    // the element lit when it is what is selected, not one of its items
    let child = items.iter().any(|it| st.item_selected(*it));
    let selected = st.is_selected(f.id) && !child && !st.selected_group().is_some_and(|g| g.members.contains(&f.id));
    let w = ui.available_width();
    let (row, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let r = Rect::from_min_max(pos2(row.min.x + indent, row.min.y), row.max);
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(("row", f.id)), r.center()));
    let arrow = Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(28.0, 40.0));
    let aresp = ui.interact(arrow, Id::new(("s-arrow", f.id)), Sense::click());
    row_look(ui, r, selected, resp.is_pointer_button_down_on());
    let dim = if f.visible { 1.0 } else { 0.45 };
    let p = ui.painter();
    if !items.is_empty() {
        icons::paint(p, Rect::from_center_size(arrow.center(), vec2(12.0, 12.0)), if open { Icon::Chevron } else { Icon::ChevronRight }, muted());
    }
    let ic = Rect::from_center_size(pos2(r.min.x + 40.0, r.center().y), vec2(17.0, 17.0));
    // its kind (a block, a tube, a sketch, an SDF object…), a dug one in
    // the carve colour
    let carved = f.is_solid() && !f.is_mesh() && f.op == Op::Carve;
    icons::paint(p, ic, feature_icon(f), tint(if selected { accent_text() } else if carved { carve() } else { ink() }, dim));
    let note = if items.is_empty() { String::new() } else { items.len().to_string() };
    row_text(ui, r, &f.name, &note, selected, selected || open, dim);
    resp.context_menu(|ui| row_menu(ui, st, f));
    if aresp.clicked() && !items.is_empty() {
        ui.data_mut(|d| d.insert_temp(id, !open));
    } else if resp.clicked() {
        element_click(ui, st, f);
    }
    if open {
        let marker = st.item_marker(f.id);
        for (k, it) in items.iter().enumerate() {
            if k == marker && marker < items.len() {
                let (mr, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_x_y_ranges(mr.min.x + indent + 26.0..=mr.max.x - 6.0, mr.center().y - 1.0..=mr.center().y + 1.0), 1.0, TEXT_STRONG());
            }
            item_line(ui, st, *it, indent + 26.0);
        }
    }
}

/// A tap on an element's row: as the classic list's (`tree_row`).
fn element_click(ui: &Ui, st: &mut AppState, f: &Feature) {
    let add = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
    if f.is_solid() {
        st.set_sel_mode(SelMode::Object);
        st.outline_focus = Some(f.id);
        st.click_shape(Some(f.id), add);
    } else {
        st.select_tool();
        st.edge_sel.clear();
        st.select(Some(f.id), add);
    }
}

/// One item of an element (an operation, an SDF shape): a small mark in
/// its colour, its name, its note (Add, Carve · 9); dimmed when off or past
/// the marker.
fn item_line(ui: &mut Ui, st: &mut AppState, it: crate::common::Item, indent: f32) {
    use crate::common::Item;
    let w = ui.available_width();
    let (row, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let r = Rect::from_min_max(pos2(row.min.x + indent, row.min.y), row.max);
    let selected = st.item_selected(it);
    let el = st.item_element(it);
    let past = el.map(|e| st.item_marker(e) <= st.element_items(e).iter().position(|x| *x == it).unwrap_or(0)).unwrap_or(false);
    let dim = if st.item_on(it) && !past { 1.0 } else { 0.45 };
    let p = ui.painter();
    if selected {
        p.rect_filled(r, 10.0, accent_bg());
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 10.0, RAISED());
    }
    // the mark: a dot (added), a dashed ring (carved), a ring (common
    // part), a bar (an operation)
    let c = pos2(r.min.x + 14.0, r.center().y);
    let (note, mark) = match it {
        Item::Shape(id, n) => {
            let nd = st.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)).cloned();
            let (mode, blend) = nd.map(|x| (x.combine.mode, x.combine.blend)).unwrap_or((forme_core::CombineMode::Add, 0.0));
            let word = match mode {
                forme_core::CombineMode::Add => "Add",
                forme_core::CombineMode::Carve => "Carve",
                forme_core::CombineMode::Intersect => "Overlap",
                m => m.label(),
            };
            let note = if blend > 0.0 { format!("{word} · {}", fmt3(blend as f64)) } else { word.to_string() };
            (note, mode)
        }
        Item::Feature(_) => (st.item_info(it).1, forme_core::CombineMode::Add),
    };
    let col = if selected { accent_text() } else { tint(item_look(st, it).1, dim) };
    match (it, mark) {
        (Item::Shape(..), forme_core::CombineMode::Carve) => {
            // dashed ring
            for k in 0..8 {
                let a0 = k as f32 * std::f32::consts::TAU / 8.0;
                let a1 = a0 + std::f32::consts::TAU / 16.0;
                p.line_segment([c + vec2(a0.cos(), a0.sin()) * 4.5, c + vec2(a1.cos(), a1.sin()) * 4.5], Stroke::new(1.5, tint(carve(), dim)));
            }
        }
        (Item::Shape(..), forme_core::CombineMode::Intersect) => {
            p.circle_stroke(c, 4.5, Stroke::new(1.5, col));
        }
        (Item::Shape(..), _) => {
            p.circle_filled(c, 4.5, col);
        }
        (Item::Feature(_), _) => {
            p.rect_filled(Rect::from_center_size(c, vec2(9.0, 3.0)), 1.5, col);
        }
    }
    let gn = p.layout_no_wrap(note, regular(12.0), if selected { accent_text() } else { tint(muted(), dim) });
    let nx = r.max.x - 8.0 - gn.size().x;
    p.galley(pos2(nx, r.center().y - gn.size().y * 0.5), gn, muted());
    let tc = if selected { accent_text() } else { tint(ink(), dim) };
    let mut job = egui::text::LayoutJob::single_section(st.item_name(it), egui::TextFormat::simple(if selected { semibold(13.0) } else { regular(13.0) }, tc));
    job.wrap = egui::text::TextWrapping { max_width: (nx - 8.0 - (r.min.x + 26.0)).max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(r.min.x + 26.0, r.center().y - g.size().y * 0.5), g, tc);
    resp.context_menu(|ui| item_menu(ui, st, it));
    if resp.clicked() {
        st.select_item(it);
    }
}

// ------------------------------------------------------------ the inspector

/// What the inspector shows: the subject of the classic operation window,
/// or the selected group.
fn inspector(ctx: &egui::Context, st: &mut AppState, screen: Rect, flip: bool) {
    let subject = op_pop(st).filter(|_| !st.marking.is_open());
    let group = subject.is_none() && st.selected_group().is_some() && !st.marking.is_open();
    if subject.is_none() && !group {
        return;
    }
    let x = if flip { screen.min.x + E } else { screen.max.x - E - INSP_W };
    let bottom = screen.max.y - E - FRAME_H - 8.0 - CUBE - 8.0 - NAV_H - 12.0;
    let max_h = (bottom - TOP - 32.0).max(160.0);
    set_op_style(ctx, true);
    egui::Area::new(Id::new("s-insp")).order(Order::Middle).fixed_pos(pos2(x, TOP)).show(ctx, |ui| {
        glass(20).fill(tint(PANEL(), 0.92)).inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(INSP_W - 32.0);
            egui::ScrollArea::vertical().id_salt("s-insp-scroll").max_height(max_h).min_scrolled_height(max_h).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded).show(ui, |ui| {
                ui.set_width(INSP_W - 32.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                match subject {
                    None => group_properties(ui, st),
                    Some(k) => subject_card(ui, st, k),
                }
            });
        });
    });
    set_op_style(ctx, false);
}

fn subject_card(ui: &mut Ui, st: &mut AppState, k: OpPop) {
    match k {
        OpPop::Shape(id) => {
            let Some(f) = st.doc.feature(id).cloned() else { return };
            if crate::sdf::is_sdf(&f) {
                match st.sdf_sel.filter(|(o, _)| *o == id && st.sdf_comp()) {
                    Some((_, n)) => sdf_shape_card(ui, st, &f, n),
                    None => sdf_object_card(ui, st, &f),
                }
            } else if plain_solid(&f) {
                solid_card(ui, st, &f);
            } else {
                op_body(ui, st, k);
            }
        }
        OpPop::Edges => round_card_s(ui, st),
        OpPop::Step(id) if st.doc.feature(id).is_some_and(|f| f.round.is_some()) => round_card_s(ui, st),
        _ => op_body(ui, st, k),
    }
}

/// A Cube, Cylinder or Sketch of the Forme tab (not a mesh, a form, a
/// curve made solid, a linked object).
fn plain_solid(f: &Feature) -> bool {
    f.is_solid() && !f.is_mesh() && !f.is_path() && f.form.is_none() && !f.is_extruded_curve() && matches!(f.prim, Prim::Box { .. } | Prim::Cylinder { .. } | Prim::Profile { .. })
}

/// The kind of a plain solid, in the mock-up's words.
fn solid_word(f: &Feature) -> &'static str {
    match f.prim {
        Prim::Box { .. } => "Block",
        Prim::Cylinder { .. } => "Tube",
        Prim::Profile { .. } => "Sketch",
        Prim::Path { .. } => "Path",
    }
}

/// A Cube, Cylinder or Sketch (the mock-up D1): Build / Dig, its sizes,
/// its material, its position folded.
fn solid_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let id = f.id;
    let mut live = false;
    header(ui, &f.name, solid_word(f), false, "");
    ui.add_space(10.0);
    if let Some(op) = seg(ui, "s-op", &[(Op::Add, "Build"), (Op::Carve, "Dig")], f.op, 40.0) {
        set_op_from_ui(st, op);
    }
    label(ui, "Size");
    match f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => {
            for (k, name, v) in [(0usize, "Width", x), (2, "Depth", z), (1, "Height", y)] {
                let mut val = v as f32;
                let (ch, start) = stepper(ui, name, &mut val, 0.1..=2000.0, 0.1, "mm", false);
                if ch {
                    feature_edit(st, id, "Size", start, |g| {
                        if let Prim::Box { x, y, z } | Prim::Profile { x, y, z } = &mut g.prim {
                            *[x, y, z][k] = val.max(0.1) as f64;
                        }
                    });
                    live = true;
                }
            }
        }
        Prim::Cylinder { radius, height } => {
            let mut d = (radius * 2.0) as f32;
            let (ch, start) = stepper(ui, "Diameter", &mut d, 0.1..=2000.0, 0.1, "mm", false);
            if ch {
                feature_edit(st, id, "Radius", start, |g| {
                    if let Prim::Cylinder { radius, .. } = &mut g.prim {
                        *radius = (d * 0.5).max(0.05) as f64;
                    }
                });
                live = true;
            }
            let mut h = height as f32;
            let (ch, start) = stepper(ui, "Height", &mut h, 0.1..=2000.0, 0.1, "mm", false);
            if ch {
                feature_edit(st, id, "Height", start, |g| g.prim.set_height(h.max(0.1) as f64));
                live = true;
            }
        }
        Prim::Path { .. } => {}
    }
    label(ui, "Material");
    swatches(ui, st);
    if fold_row(ui, "Position and turn", "solid-pos") {
        position_rows(ui, st, f);
    }
    // a plain Cube or Cylinder made a linked object (the PC)
    if st.linkable_solid().map(|g| g.id) == Some(id) && !st.manual_only {
        ui.add_space(6.0);
        if wide_btn(ui, "Linked object", ui.available_width(), false).clicked() {
            st.make_linked_solid();
        }
    }
    if !any_down && !live && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// Picked edges, or a selected fillet or chamfer (the mock-up D2):
/// Round / Chamfer, its size, its shape (the tension), Blend the corners,
/// Cancel / Done.
fn round_card_s(ui: &mut Ui, st: &mut AppState) {
    let any_down = ui.input(|i| i.pointer.any_down());
    let existing = st.selected_round().map(|f| (f.id, f.round.clone()));
    let existing = existing.and_then(|(id, r)| r.map(|r| (id, r)));
    let (size, chamfer, n) = match &existing {
        Some((_, r)) => (r.size, r.chamfer, r.edges.len()),
        None => (0.0, st.round_chamfer, st.edge_sel.len()),
    };
    header(ui, if chamfer { "Chamfer" } else { "Round" }, &format!("{n} edge{}", if n > 1 { "s" } else { "" }), false, "");
    ui.add_space(10.0);
    if let Some(c) = seg(ui, "s-round-kind", &[(false, "Round"), (true, "Chamfer")], chamfer, 40.0) {
        if existing.is_some() {
            st.set_round(size, c, true);
            st.end_live();
        } else {
            st.round_chamfer = c;
        }
    }
    label(ui, if chamfer { "Distance" } else { "Size" });
    let max = st.round_max().max(size.ceil()) as f32;
    let mut v = size as f32;
    let mut live = false;
    let (ch, start) = stepper(ui, "", &mut v, 0.0..=max.max(0.5), 0.1, "mm", true);
    if ch {
        st.set_round(v.max(0.0) as f64, chamfer, start || !st.is_live());
        live = true;
    }
    let w = ui.available_width();
    let mut sv = v;
    let (ch, started) = touch_slider(ui, Id::new("s-round-slider"), &mut sv, 0.0, 10.0f32.min(max.max(0.5)), true, 0.1, w);
    if ch && (sv - v).abs() > 1e-5 {
        st.set_round(sv.max(0.0) as f64, chamfer, started || !st.is_live());
        live = true;
    }
    // one tap sizes
    let quick: Vec<(f64, String, bool)> = [0.5, 1.0, 2.0, 3.0, 5.0].iter().map(|q| (*q, fmt_mm(*q as f32), (size - q).abs() < 1e-6)).collect();
    let items: Vec<(f64, &str, bool)> = quick.iter().map(|(q, s, on)| (*q, s.as_str(), *on)).collect();
    ui.add_space(2.0);
    if let Some(q) = pills(ui, "s-round-quick", &items, 34.0) {
        st.set_round(q, chamfer, true);
        st.end_live();
    }
    if let Some((id, r)) = existing.as_ref().filter(|(_, r)| !r.chamfer) {
        label(ui, "Shape");
        let circle = nurbs_kernel::doc::circle_rho(std::f64::consts::FRAC_PI_2);
        let cur = if r.tension == 0.0 { 1 } else if r.tension < circle { 0 } else { 2 };
        if let Some(k) = seg(ui, "s-round-shape", &[(0, "Flatter"), (1, "Circular"), (2, "Fuller")], cur, 40.0) {
            let t = [0.25, 0.0, 0.62][k];
            st.set_round_tension(*id, t, true);
            st.end_live();
        }
        if !r.only {
            ui.add_space(4.0);
            if switch_row(ui, "Blend the corners", r.blend) {
                st.set_blend(!r.blend);
            }
        }
    }
    let only = existing.as_ref().map(|(_, r)| r.only).unwrap_or(st.round_only);
    if switch_row(ui, "Only the edges picked", only) {
        st.set_round_only(!only);
    }
    if st.reference.is_some() && existing.as_ref().map(|(_, r)| !r.chamfer).unwrap_or(true) {
        ui.add_space(4.0);
        if wide_btn(ui, "Fit to scan", ui.available_width(), false).clicked() {
            st.fit_round_to_scan();
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let bw = (ui.available_width() - 8.0) * 0.5;
        if wide_btn(ui, "Cancel", bw, false).clicked() {
            if existing.is_some() {
                st.delete_selected();
            }
            st.edge_sel.clear();
        }
        if wide_btn(ui, "Done", bw, true).clicked() {
            st.end_live();
            st.selection.clear();
            st.edge_sel.clear();
        }
    });
    if !any_down && !live && st.is_live() && st.drag.is_none() && st.creation.is_none() {
        st.end_live();
    }
}

/// An SDF object with no shape picked: its shapes, a tap picks one.
fn sdf_object_card(ui: &mut Ui, st: &mut AppState, f: &Feature) {
    let items = st.element_items(f.id);
    header(ui, &f.name, "Soft object", true, &format!("{} shape{}", items.len(), if items.len() == 1 { "" } else { "s" }));
    label(ui, "Shapes");
    for it in items {
        item_line(ui, st, it, 0.0);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Tap a shape, here or on the part, to change it").font(regular(12.5)).color(TEXT2()));
    label(ui, "Material");
    swatches(ui, st);
}

/// A shape of an SDF object (the mock-up D3): how it joins, the blend, its
/// sizes, the mirror, its material; its position, turn and repeat folded.
fn sdf_shape_card(ui: &mut Ui, st: &mut AppState, f: &Feature, n: forme_core::NodeId) {
    use forme_core::{Axis, BlendKind, CombineMode, PrimKind, Repeat};
    let Some(scene) = crate::sdf::scene_of(f).cloned() else { return };
    let Some(nd) = scene.node(n).cloned() else { return };
    let Some(sh) = nd.shape().cloned() else { return };
    let any_down = ui.input(|i| i.pointer.any_down());
    let mut live = false;
    header(ui, &nd.name, "Soft shape", true, &format!("in {}", f.name));
    ui.add_space(10.0);
    let mode = if crate::sdf::SDF_MODES.contains(&nd.combine.mode) { nd.combine.mode } else { CombineMode::Add };
    if let Some(m) = seg(ui, "s-sdf-mode", &[(CombineMode::Add, "Add"), (CombineMode::Carve, "Carve"), (CombineMode::Intersect, "Overlap")], mode, 40.0) {
        st.sdf.mode = m;
        st.sdf_edit_node(&format!("Make it {}", m.label()), |x| x.combine.mode = m);
    }
    label(ui, "Blend with the others");
    let mut b = nd.combine.blend;
    if stepper(ui, "", &mut b, 0.0..=30.0, 0.1, "mm", true).0 && (b - nd.combine.blend).abs() > 1e-5 {
        st.sdf_live_node("Blend", |x| x.combine.blend = b);
        live = true;
    }
    if nd.combine.blend > 0.0 {
        ui.add_space(2.0);
        if let Some(k) = pills(ui, "s-blend-kind", &[(BlendKind::Smooth, "Round", nd.combine.kind == BlendKind::Smooth), (BlendKind::Chamfer, "Chamfer", nd.combine.kind == BlendKind::Chamfer)], 34.0) {
            st.sdf_edit_node("Blend kind", |x| x.combine.kind = k);
        }
    }
    label(ui, "Size");
    // the engine's sizes are half sizes: shown whole
    let rows: Vec<(&str, usize)> = match sh.kind {
        PrimKind::Box => vec![("Width", 0), ("Height", 1), ("Depth", 2)],
        PrimKind::Cylinder => vec![("Diameter", 0), ("Height", 1)],
        PrimKind::Capsule => vec![("Length", 1), ("Diameter", 0)],
        PrimKind::Sphere => vec![("Diameter", 0)],
        PrimKind::Torus => vec![("Diameter", 0), ("Thickness", 2)],
        _ => vec![("Size", 0)],
    };
    for (name, a) in rows {
        let was = sh.size[a] * 2.0;
        let mut v = was;
        if stepper(ui, name, &mut v, 0.2..=500.0, 0.1, "mm", false).0 && (v - was).abs() > 1e-5 {
            let k = sh.kind;
            st.sdf_live_node("Size", |x| {
                if let Some(s) = x.shape_mut() {
                    if k == PrimKind::Sphere {
                        s.size = glam::Vec3::splat(v * 0.5);
                    } else {
                        s.size[a] = v * 0.5;
                    }
                }
            });
            live = true;
        }
    }
    if matches!(sh.kind, PrimKind::Box | PrimKind::Cylinder) {
        let mut v = sh.round;
        if stepper(ui, "Round", &mut v, 0.0..=50.0, 0.1, "mm", false).0 && (v - sh.round).abs() > 1e-5 {
            st.sdf_live_node("Fillet", |x| {
                if let Some(s) = x.shape_mut() {
                    s.round = v;
                    s.edges.clear();
                }
            });
            live = true;
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (lr, _) = ui.allocate_exact_size(vec2(72.0, 36.0), Sense::hover());
        ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, "Mirror", regular(13.0), ink());
        let on = [nd.mirror.x, nd.mirror.y, nd.mirror.z];
        if let Some(a) = pills(ui, "s-mirror", &[(0usize, "X", on[0]), (1, "Y", on[1]), (2, "Z", on[2])], 36.0) {
            let name = ["X", "Y", "Z"][a];
            st.sdf_edit_node(&format!("Mirror {name}"), |x| match a {
                0 => x.mirror.x = !on[0],
                1 => x.mirror.y = !on[1],
                _ => x.mirror.z = !on[2],
            });
        }
    });
    label(ui, "Material");
    swatches(ui, st);
    if fold_row(ui, "Position, turn and repeat", "sdf-pos") {
        if let Some(pos) = xyz_fields(ui, ["X", "Y", "Z"], nd.transform.pos.to_array(), "") {
            st.sdf_live_node("Move shape", |x| x.transform.pos = glam::Vec3::from(pos));
            live = true;
        }
        let (ex, ey, ez) = nd.transform.rot.to_euler(glam::EulerRot::XYZ);
        if let Some(e) = xyz_fields(ui, ["rX", "rY", "rZ"], [ex.to_degrees(), ey.to_degrees(), ez.to_degrees()], "°") {
            st.sdf_live_node("Rotate shape", |x| x.transform.rot = glam::Quat::from_euler(glam::EulerRot::XYZ, e[0].to_radians(), e[1].to_radians(), e[2].to_radians()));
            live = true;
        }
        let kind = match nd.repeat {
            Repeat::None => 0,
            Repeat::Radial { .. } => 1,
            Repeat::Linear { .. } => 2,
        };
        ui.add_space(4.0);
        if let Some(k) = seg(ui, "s-repeat", &[(0, "No repeat"), (1, "Round"), (2, "Along")], kind, 40.0) {
            let r = match k {
                1 => Repeat::Radial { count: 6, axis: Axis::Y },
                2 => Repeat::Linear { count: 3, step: glam::Vec3::new((sh.local_half_extents().x * 2.0 + 2.0).max(4.0), 0.0, 0.0) },
                _ => Repeat::None,
            };
            st.sdf_edit_node("Repeat", |x| x.repeat = r);
        }
        match nd.repeat {
            Repeat::Radial { count, axis } => {
                let mut c = count as f32;
                if stepper(ui, "Copies", &mut c, 2.0..=48.0, 1.0, "", false).0 && c.round() as u32 != count {
                    let c = c.round().max(2.0) as u32;
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Radial { count: c, axis });
                    live = true;
                }
                if let Some(ax) = pills(ui, "s-rep-axis", &[(Axis::X, "Round X", axis == Axis::X), (Axis::Y, "Round Y", axis == Axis::Y), (Axis::Z, "Round Z", axis == Axis::Z)], 34.0) {
                    st.sdf_edit_node("Repeat", |x| x.repeat = Repeat::Radial { count, axis: ax });
                }
            }
            Repeat::Linear { count, step } => {
                let mut c = count as f32;
                if stepper(ui, "Copies", &mut c, 2.0..=48.0, 1.0, "", false).0 && c.round() as u32 != count {
                    let c = c.round().max(2.0) as u32;
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Linear { count: c, step });
                    live = true;
                }
                if let Some(s) = xyz_fields(ui, ["X", "Y", "Z"], step.to_array(), "") {
                    st.sdf_live_node("Repeat", |x| x.repeat = Repeat::Linear { count, step: glam::Vec3::from(s) });
                    live = true;
                }
            }
            Repeat::None => {}
        }
    }
    if !any_down && !live && st.is_live() && st.sdf_drag.is_none() {
        st.end_live();
    }
}

// ------------------------------------------------------------ the dock

/// A tool of the dock: its icon over its word (66 × 72), lit in the
/// accent's tint. Its rect for a custom icon.
fn dock_btn(ui: &mut Ui, icon: Option<Icon>, word: &str, on: bool) -> (Response, Rect) {
    let (r, resp) = ui.allocate_exact_size(vec2(66.0, 72.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 14.0, accent_bg());
    } else if resp.is_pointer_button_down_on() || resp.hovered() {
        p.rect_filled(r, 14.0, RAISED());
    }
    let c = if on { accent_text() } else { TEXT() };
    if let Some(i) = icon {
        icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 10.0), vec2(22.0, 22.0)), i, c);
    }
    p.text(pos2(r.center().x, r.center().y + 17.0), Align2::CENTER_CENTER, word, if on { semibold(12.0) } else { medium(12.0) }, c);
    (resp, r)
}

/// A tap on a dock button closes the fly-outs it does not open.
fn close_flies(ctx: &egui::Context, keep: &str) {
    for n in ["s-shapes", "s-more", "mats"] {
        if n != keep {
            set_flag(ctx, n, false);
        }
    }
}

/// The tools, in a dock at the bottom: the tab's own, simple words.
fn dock(ctx: &egui::Context, st: &mut AppState, screen: Rect) {
    egui::Area::new(Id::new("s-dock")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(screen.center().x, screen.max.y - E)).show(ctx, |ui| {
        glass(24).fill(tint(PANEL(), 0.9)).shadow(shadow(true)).inner_margin(Margin::symmetric(10, 9)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // the tab's tools at once (22 nov.: faded in, the dock read
                // as empty for a moment after a tap on Crisp or Soft)
                match st.workspace {
                    Workspace::Sdf => soft_tools(ctx, ui, st),
                    Workspace::Sculpt => sculpt_tools(ctx, ui, st),
                    Workspace::Poly => poly_tools(ctx, ui, st),
                    Workspace::Forme => crisp_tools(ctx, ui, st),
                }
                bar_sep(ui, 72.0);
                // Paint: the six balls above it
                let cur = current_material(st).unwrap_or(st.material);
                let open = flag(ctx, "mats");
                let (resp, r) = dock_btn(ui, None, "Paint", open);
                material_ball(ui.painter(), pos2(r.center().x, r.center().y - 10.0), 11.0, &st.look(cur));
                ctx.data_mut(|d| d.insert_temp(Id::new("s-paint-btn"), r));
                if resp.clicked() {
                    close_flies(ctx, "mats");
                    set_flag(ctx, "mats", !open);
                }
                if st.workspace == Workspace::Forme {
                    let open = flag(ctx, "s-more");
                    let (resp, r) = dock_btn(ui, Some(Icon::Array), "More", open || matches!(st.tool, Tool::Align | Tool::Push(PushKind::Inset) | Tool::Point));
                    ctx.data_mut(|d| d.insert_temp(Id::new("s-more-btn"), r));
                    if resp.clicked() {
                        close_flies(ctx, "s-more");
                        set_flag(ctx, "s-more", !open);
                    }
                }
            });
        });
    });
}

/// The Select button of every tab: back to picking whole shapes.
fn select_btn(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    let on = st.tool == Tool::Select && st.sel_mode == SelMode::Object;
    if dock_btn(ui, Some(Icon::Select), "Select", on).0.clicked() {
        close_flies(ctx, "");
        st.select_tool();
        st.set_sel_mode(SelMode::Object);
    }
}

/// Crisp's Select: the four selection modes in a square (Romain, 22 nov.,
/// 18 h 05: "Où sont les modes de sélection Components ?"), Object and
/// Points above, Edges and Faces under them; each tap goes back to Select.
fn mode_grid(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    let (wr, _) = ui.allocate_exact_size(vec2(84.0, 72.0), Sense::hover());
    ui.painter().rect_filled(wr, 14.0, seg_bg());
    let modes = [(SelMode::Object, Icon::ModeObject, "Object: whole shapes"), (SelMode::Vertex, Icon::ModeVertex, "Points"), (SelMode::Edge, Icon::ModeEdge, "Edges"), (SelMode::Face, Icon::ModeFace, "Faces")];
    for (i, (m, icon, tip)) in modes.into_iter().enumerate() {
        let r = Rect::from_min_size(wr.min + vec2(4.0 + (i % 2) as f32 * 39.0, 4.0 + (i / 2) as f32 * 33.0), vec2(37.0, 31.0));
        let resp = ui.interact(r, Id::new(("s-selmode", i)), Sense::click());
        let on = st.tool == Tool::Select && st.sel_mode == m;
        let p = ui.painter();
        if on {
            p.rect_filled(r.translate(vec2(0.0, 1.0)), 10.0, Color32::from_black_alpha(if light() { 18 } else { 60 }));
            p.rect_filled(r, 10.0, accent_bg());
        } else if resp.is_pointer_button_down_on() {
            p.rect_filled(r, 10.0, seg_on());
        }
        icons::paint(p, Rect::from_center_size(r.center(), vec2(19.0, 19.0)), icon, if on { accent_text() } else { TEXT2() });
        if resp.on_hover_text(tip).clicked() {
            close_flies(ctx, "");
            st.select_tool();
            st.set_sel_mode(m);
        }
    }
}

/// Crisp (the Forme tab, the mock-up D1): Build / Dig, Select, Block,
/// Tube, Sketch, Shapes, then Round, Push, Cut.
fn crisp_tools(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    // Build / Dig: what the next shapes do, only (Romain, 22 nov., 17 h 54:
    // "ADD et SUBB … semblait pas fonctionner": a shape just made stays
    // picked, so Dig tapped next turned that shape into a dig and the part
    // vanished; a picked shape's own Build / Dig is in the inspector)
    let shown = st.mode;
    let (wr, _) = ui.allocate_exact_size(vec2(76.0, 72.0), Sense::hover());
    ui.painter().rect_filled(wr, 14.0, seg_bg());
    for (i, (op, word)) in [(Op::Add, "Build"), (Op::Carve, "Dig")].into_iter().enumerate() {
        let r = Rect::from_min_size(wr.min + vec2(4.0, 4.0 + i as f32 * 33.0), vec2(68.0, 31.0));
        let resp = ui.interact(r, Id::new(("s-builddig", i)), Sense::click());
        let on = shown == op;
        let p = ui.painter();
        if on {
            p.rect_filled(r.translate(vec2(0.0, 1.0)), 10.0, Color32::from_black_alpha(if light() { 18 } else { 60 }));
            p.rect_filled(r, 10.0, seg_on());
        }
        let c = if on { if op == Op::Carve { carve_light() } else { TEXT_STRONG() } } else { TEXT2() };
        p.text(r.center(), Align2::CENTER_CENTER, word, if on { semibold(12.0) } else { medium(12.0) }, c);
        if resp.on_hover_text(if op == Op::Carve { "The next shapes dig into the part" } else { "The next shapes build onto the part" }).clicked() {
            st.mode = op;
        }
    }
    ui.add_space(4.0);
    mode_grid(ctx, ui, st);
    bar_sep(ui, 72.0);
    for (k, word) in [(PrimKind::Box, "Block"), (PrimKind::Cylinder, "Tube"), (PrimKind::Profile, "Sketch")] {
        if dock_btn(ui, Some(prim_icon(k)), word, st.tool == Tool::Create(k)).0.clicked() {
            close_flies(ctx, "");
            st.arm(k);
        }
    }
    let open = flag(ctx, "s-shapes");
    let more = matches!(st.tool, Tool::Curve(_) | Tool::Slot | Tool::Form | Tool::Create(PrimKind::Surface) | Tool::Create(PrimKind::Path));
    let (resp, r) = dock_btn(ui, Some(Icon::CurveRect), "Shapes", open || more);
    ctx.data_mut(|d| d.insert_temp(Id::new("s-shapes-btn"), r));
    if resp.clicked() {
        close_flies(ctx, "s-shapes");
        set_flag(ctx, "s-shapes", !open);
    }
    bar_sep(ui, 72.0);
    let round = st.tool == Tool::Select && st.sel_mode == SelMode::Edge;
    if dock_btn(ui, Some(Icon::Fillet), "Round", round).0.clicked() {
        close_flies(ctx, "");
        st.select_tool();
        st.set_sel_mode(SelMode::Edge);
    }
    if dock_btn(ui, Some(Icon::Extrude), "Push", st.tool == Tool::Push(PushKind::Extrude)).0.clicked() {
        close_flies(ctx, "");
        st.arm_push(PushKind::Extrude, None);
    }
    if dock_btn(ui, Some(Icon::Knife), "Cut", st.tool == Tool::Knife).0.clicked() {
        close_flies(ctx, "");
        st.arm_knife(Tool::Knife);
    }
}

/// Soft (the SDF tab, the mock-up D3): Select, Ball, Block, Tube, Bean,
/// Ring, then Add / Carve / Overlap.
fn soft_tools(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    use forme_core::{CombineMode, PrimKind as K};
    select_btn(ctx, ui, st);
    bar_sep(ui, 72.0);
    let picked = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).map(|nd| (nd.shape().map(|s| s.kind), nd.combine.mode)));
    for (k, word) in [(K::Sphere, "Ball"), (K::Box, "Block"), (K::Cylinder, "Tube"), (K::Capsule, "Bean"), (K::Torus, "Ring")] {
        let on = picked.and_then(|p| p.0) == Some(k);
        if dock_btn(ui, Some(crate::sdf::prim_icon(k)), word, on).0.clicked() {
            close_flies(ctx, "");
            st.sdf_add(k);
        }
    }
    bar_sep(ui, 72.0);
    // what the next shapes do, only (a picked shape's own mode is in the
    // inspector; see Crisp's Build / Dig)
    let mode = st.sdf.mode;
    let (wr, _) = ui.allocate_exact_size(vec2(4.0 * 2.0 + 3.0 * 60.0 + 2.0 * 3.0, 72.0), Sense::hover());
    ui.painter().rect_filled(wr, 14.0, seg_bg());
    for (i, (m, icon, word)) in [(CombineMode::Add, Icon::BoolUnion, "Add"), (CombineMode::Carve, Icon::BoolSubtract, "Carve"), (CombineMode::Intersect, Icon::BoolIntersect, "Overlap")].into_iter().enumerate() {
        let r = Rect::from_min_size(wr.min + vec2(4.0 + i as f32 * 63.0, 4.0), vec2(60.0, 64.0));
        let resp = ui.interact(r, Id::new(("s-sdfmode", i)), Sense::click());
        let on = mode == m;
        let p = ui.painter();
        if on {
            p.rect_filled(r.translate(vec2(0.0, 1.0)), 10.0, Color32::from_black_alpha(if light() { 18 } else { 60 }));
            p.rect_filled(r, 10.0, seg_on());
        }
        let c = if on { if m == CombineMode::Carve { carve_light() } else { TEXT_STRONG() } } else { TEXT2() };
        icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 9.0), vec2(18.0, 18.0)), icon, c);
        p.text(pos2(r.center().x, r.center().y + 15.0), Align2::CENTER_CENTER, word, if on { semibold(12.0) } else { medium(12.0) }, c);
        if resp.clicked() {
            st.sdf.mode = m;
        }
    }
}

/// Sculpt (the PC): Select, a clay sphere or block, the brushes.
fn sculpt_tools(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    use crate::sculpt::Brush;
    select_btn(ctx, ui, st);
    bar_sep(ui, 72.0);
    for (block, icon, word) in [(false, Icon::Sphere, "Clay ball"), (true, Icon::Box, "Clay block")] {
        if dock_btn(ui, Some(icon), word, false).0.clicked() {
            close_flies(ctx, "");
            st.add_sculpt(block);
        }
    }
    bar_sep(ui, 72.0);
    for b in Brush::ALL {
        if dock_btn(ui, Some(b.icon()), b.label(), st.tool == Tool::Sculpt && st.sculpt.brush == b).0.clicked() {
            close_flies(ctx, "");
            st.set_brush(b);
        }
    }
    if dock_btn(ui, Some(Icon::Mirror), "Mirror", st.sculpt.mirror).0.clicked() {
        st.sculpt.mirror = !st.sculpt.mirror;
    }
}

/// Poly (the PC): Select, the meshes, the face and edge tools.
fn poly_tools(ctx: &egui::Context, ui: &mut Ui, st: &mut AppState) {
    use crate::poly::{PolyShape, PullKind};
    select_btn(ctx, ui, st);
    bar_sep(ui, 72.0);
    for (k, icon) in [(PolyShape::Cube, Icon::Box), (PolyShape::Cylinder, Icon::Cylinder), (PolyShape::Sphere, Icon::Sphere), (PolyShape::Plane, Icon::PolyPlane)] {
        if dock_btn(ui, Some(icon), k.label(), false).0.clicked() {
            close_flies(ctx, "");
            st.add_poly(k);
        }
    }
    bar_sep(ui, 72.0);
    let at = st.principal();
    let pulling = |st: &AppState, k: PullKind| st.poly_pull.as_ref().map(|g| g.kind == k).unwrap_or(false);
    if dock_btn(ui, Some(Icon::Extrude), "Extrude", pulling(st, PullKind::Extrude)).0.clicked() {
        st.begin_poly_pull(PullKind::Extrude, at);
    }
    if dock_btn(ui, Some(Icon::Inset), "Inset", pulling(st, PullKind::Inset)).0.clicked() {
        st.begin_poly_pull(PullKind::Inset, at);
    }
    if dock_btn(ui, Some(Icon::PolyBevel), "Bevel", st.poly_bevel.as_ref().map(|b| b.live).unwrap_or(false)).0.clicked() {
        st.begin_poly_bevel(at);
    }
    if dock_btn(ui, Some(Icon::EdgeLoop), "Loop", st.tool == Tool::PolyLoop).0.clicked() {
        st.arm_poly_loop();
    }
    if dock_btn(ui, Some(Icon::PolyCut), "Cut", st.tool == Tool::PolyCut).0.clicked() {
        st.arm_poly_cut();
    }
    if dock_btn(ui, Some(Icon::Subdivide), "Smooth", false).0.clicked() {
        st.poly_subdivide();
    }
}

/// A row of tools above a dock button (its rect's key); the index tapped.
fn fly(ctx: &egui::Context, name: &str, btn: &str, items: &[(Icon, &str, bool, bool)]) -> Option<usize> {
    let Some(btn) = ctx.data(|d| d.get_temp::<Rect>(Id::new(btn))) else { return None };
    let mut hit = None;
    let area = egui::Area::new(Id::new(("s-fly", name))).order(Order::Foreground).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(btn.center().x, btn.min.y - 22.0)).show(ctx, |ui| {
        pop_glass(18).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (icon, word, on, line)) in items.iter().enumerate() {
                    if *line {
                        bar_sep(ui, 64.0);
                    }
                    let (r, resp) = ui.allocate_exact_size(vec2(76.0, 64.0), Sense::click());
                    let p = ui.painter();
                    if *on {
                        p.rect_filled(r, 12.0, accent_bg());
                    } else if resp.is_pointer_button_down_on() || resp.hovered() {
                        p.rect_filled(r, 12.0, RAISED());
                    }
                    let c = if *on { accent_text() } else { TEXT() };
                    icons::paint(p, Rect::from_center_size(pos2(r.center().x, r.center().y - 9.0), vec2(20.0, 20.0)), *icon, c);
                    p.text(pos2(r.center().x, r.center().y + 16.0), Align2::CENTER_CENTER, *word, medium(12.0), c);
                    if resp.clicked() {
                        hit = Some(i);
                    }
                }
            });
        });
    });
    if hit.is_some() || pressed_outside(ctx, &[area.response.rect, btn]) {
        set_flag(ctx, name, false);
    }
    hit
}

/// Above Shapes: the 3ds Max shapes, the Path, the Surface, the Slot, the
/// Forms.
fn shapes_fly(ctx: &egui::Context, st: &mut AppState) {
    if !flag(ctx, "s-shapes") || st.workspace != Workspace::Forme {
        return;
    }
    let kinds = [(CurveKind::Rectangle, Icon::CurveRect, "Rectangle"), (CurveKind::Circle, Icon::CurveCircle, "Circle"), (CurveKind::Ellipse, Icon::CurveEllipse, "Ellipse"), (CurveKind::Polygon, Icon::CurvePolygon, "Polygon"), (CurveKind::Star, Icon::CurveStar, "Star")];
    let mut items: Vec<(Icon, &str, bool, bool)> = kinds.iter().map(|(k, i, l)| (*i, *l, st.tool == Tool::Curve(*k), false)).collect();
    items.push((Icon::Path, "Path", st.tool == Tool::Create(PrimKind::Path), true));
    items.push((Icon::Path, "Surface", st.tool == Tool::Create(PrimKind::Surface), false));
    items.push((Icon::Scan, "Slot", st.tool == Tool::Slot, false));
    items.push((Icon::Sphere, "Forms", st.tool == Tool::Form, true));
    match fly(ctx, "s-shapes", "s-shapes-btn", &items) {
        Some(i) if i < kinds.len() => st.arm_curve(kinds[i].0),
        Some(5) => st.arm(PrimKind::Path),
        Some(6) => st.arm(PrimKind::Surface),
        Some(7) => st.arm_slot(),
        Some(8) => st.arm_form(),
        _ => {}
    }
}

/// Above More: Inset, Point on edge, the Face and Vertex modes, Hollow,
/// Mirror, Repeat, Align.
fn more_fly(ctx: &egui::Context, st: &mut AppState) {
    if !flag(ctx, "s-more") || st.workspace != Workspace::Forme {
        return;
    }
    let sel = st.tool == Tool::Select;
    let items = [
        (Icon::Inset, "Inset", st.tool == Tool::Push(PushKind::Inset), false),
        (Icon::EdgePoint, "Point", st.tool == Tool::Point, false),
        (Icon::ModeFace, "Faces", sel && st.sel_mode == SelMode::Face, true),
        (Icon::ModeVertex, "Points", sel && st.sel_mode == SelMode::Vertex, false),
        (Icon::Shell, "Hollow", false, true),
        (Icon::Mirror, "Mirror", false, false),
        (Icon::Array, "Repeat", false, false),
        (Icon::Align, "Align", st.tool == Tool::Align, false),
    ];
    match fly(ctx, "s-more", "s-more-btn", &items) {
        Some(0) => st.arm_push(PushKind::Inset, None),
        Some(1) => st.arm_knife(Tool::Point),
        Some(2) => {
            st.select_tool();
            st.set_sel_mode(SelMode::Face);
        }
        Some(3) => {
            st.select_tool();
            st.set_sel_mode(SelMode::Vertex);
        }
        Some(4) => {
            st.add_shell();
        }
        Some(5) => {
            st.add_mirror(0);
        }
        Some(6) => {
            st.add_array();
        }
        Some(7) => st.begin_align(),
        _ => {}
    }
}

/// Above Paint: the six balls and the current one's settings.
fn paint_pop(ctx: &egui::Context, st: &mut AppState) {
    if !flag(ctx, "mats") {
        return;
    }
    let Some(btn) = ctx.data(|d| d.get_temp::<Rect>(Id::new("s-paint-btn"))) else { return };
    materials_island_at(ctx, st, pos2(btn.center().x, btn.min.y - 22.0), Align2::CENTER_BOTTOM, 300.0);
    // closed by a tap in the view (a ball dragged onto the part keeps it)
    if ctx.input(|i| i.pointer.any_pressed()) && !ctx.is_pointer_over_egui() && st.mat_drag.is_none() {
        set_flag(ctx, "mats", false);
    }
}

// ------------------------------------------------------------ the view

/// The view cube and Frame, at the bottom corner on the inspector's side.
/// The navigation column at the bottom right (Romain, 22 nov.: "La
/// navigation doit pouvoir se faire full stylet", for the iPad): Zoom and
/// Pan grips dragged by the Pencil's tip alone (a finger too), the view
/// cube (a drag turns the view, a tap on a face looks square at it), Frame.
fn view_cube(ctx: &egui::Context, st: &mut AppState, screen: Rect, flip: bool) {
    let x = if flip { screen.min.x + E } else { screen.max.x - E - CUBE };
    let y = screen.max.y - E - FRAME_H - 8.0 - CUBE - 8.0 - NAV_H;
    egui::Area::new(Id::new("s-cube")).order(Order::Middle).fixed_pos(pos2(x, y)).show(ctx, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        // the grips' pill
        let (pill, _) = ui.allocate_exact_size(vec2(CUBE, NAV_H), Sense::hover());
        {
            let p = ui.painter();
            shadowed(p, pill, 18.0, tint(PANEL(), 0.88), shadow(false));
            p.rect_stroke(pill, 18.0, Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
        }
        let zr = Rect::from_center_size(pos2(pill.center().x, pill.min.y + 4.0 + GRIP * 0.5), vec2(GRIP, GRIP));
        let pr = zr.translate(vec2(0.0, GRIP + 4.0));
        let zoom = nav_grip(ui, "s-nav-zoom", zr, Icon::Zoom).on_hover_text("Zoom: drag up to come closer, down to go back");
        if zoom.dragged() {
            let d = grip_moved(ui, &zoom);
            if d != egui::Vec2::ZERO {
                st.stop_flight();
                st.camera.zoom(((d.x - d.y) * 0.006).exp());
            }
            st.interacting = true;
        }
        let pan = nav_grip(ui, "s-nav-pan", pr, Icon::PanView).on_hover_text("Pan: drag to move the view with the Pencil");
        if pan.dragged() {
            let d = grip_moved(ui, &pan);
            if d != egui::Vec2::ZERO {
                st.stop_flight();
                let h = st.view_rect[3] - st.view_rect[1];
                st.camera.pan(d.x * 1.5, d.y * 1.5, h);
            }
            st.interacting = true;
        }
        ctx.data_mut(|m| m.insert_temp(Id::new("s-nav-rects"), [zr, pr]));
        let (r, resp) = ui.allocate_exact_size(vec2(CUBE, CUBE), Sense::click_and_drag());
        ctx.data_mut(|m| m.insert_temp(Id::new("s-nav-cube"), r));
        if resp.dragged() {
            let d = grip_moved(ui, &resp);
            if d != egui::Vec2::ZERO {
                st.stop_flight();
                st.camera.orbit(d.x * 1.5, d.y * 1.5);
            }
            st.interacting = true;
        }
        let p = ui.painter();
        shadowed(&p, r, 18.0, if resp.dragged() { RAISED() } else { tint(PANEL(), 0.88) }, shadow(false));
        p.rect_stroke(r, 18.0, Stroke::new(1.0, if resp.dragged() { accent() } else { PANEL_BORDER() }), StrokeKind::Inside);
        // the cube as the camera sees it: its faces towards the eye, the
        // nearest drawn last
        let d = st.camera.dir();
        let fwd = -d;
        let mut right = fwd.cross(glam::Vec3::Y);
        if right.length_squared() < 1e-6 {
            right = glam::Vec3::X;
        }
        let right = right.normalize();
        let up = right.cross(fwd).normalize();
        let c = r.center();
        let s = 10.5;
        let to2 = |v: glam::Vec3| c + vec2(v.dot(right), -v.dot(up)) * s;
        let faces: [(glam::Vec3, &str); 6] = [(glam::Vec3::Z, "Front"), (glam::Vec3::NEG_Z, "Back"), (glam::Vec3::X, "Right"), (glam::Vec3::NEG_X, "Left"), (glam::Vec3::Y, "Top"), (glam::Vec3::NEG_Y, "Bottom")];
        let mut seen: Vec<(f32, Vec<egui::Pos2>, glam::Vec3, &str)> = vec![];
        for (n, name) in faces {
            let k = n.dot(d);
            if k <= 0.02 {
                continue;
            }
            // the face's corners: two axes across its normal
            let (a, b) = if n.x.abs() > 0.5 { (glam::Vec3::Y, glam::Vec3::Z) } else if n.y.abs() > 0.5 { (glam::Vec3::X, glam::Vec3::Z) } else { (glam::Vec3::X, glam::Vec3::Y) };
            let q: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|(u, v)| to2(n + a * *u + b * *v)).collect();
            seen.push((k, q, n, name));
        }
        seen.sort_by(|a, b| a.0.total_cmp(&b.0));
        let base = pick(rgb(0x3A, 0x3F, 0x47), rgb(0xE3, 0xE7, 0xEC));
        let lit = pick(rgb(0x55, 0x5B, 0x65), rgb(0xFA, 0xFB, 0xFC));
        // the face under the pointer (else the one the view looks at most)
        // tinted in the accent, as the mock-up's top face
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|h| r.contains(*h)).filter(|_| !resp.dragged());
        let lit_face = hover.and_then(|h| seen.iter().rev().find(|(_, q, _, _)| inside(q, h)).map(|x| x.2)).or_else(|| seen.last().map(|x| x.2));
        let edge = pick(rgb(0xA3, 0xA9, 0xB2), rgb(0x8E, 0x95, 0x9F));
        for (k, q, n, _) in &seen {
            let t = if n.y > 0.5 { 1.0 } else { (*k * 0.8).clamp(0.0, 1.0) };
            let mut col = lerp_color(base, lit, t);
            if Some(*n) == lit_face {
                col = lerp_color(col, accent(), 0.35);
            }
            p.add(egui::Shape::convex_polygon(q.clone(), col, Stroke::new(1.2, edge)));
        }
        if let Some((_, _, n, name)) = seen.iter().find(|x| Some(x.2) == lit_face).filter(|_| hover.is_some()) {
            let _ = n;
            p.text(pos2(r.center().x, r.max.y - 7.0), Align2::CENTER_BOTTOM, *name, medium(9.0), TEXT2());
        }
        if resp.on_hover_text("Drag: turn the view · Tap a face: look square at it (again: back to the perspective)").clicked() {
            if let Some(at) = ui.input(|i| i.pointer.interact_pos()) {
                // the nearest face under the tap
                if let Some((_, _, n, _)) = seen.iter().rev().find(|(_, q, _, _)| inside(q, at)) {
                    look_from(st, *n);
                }
            }
        }
        let (fr, fresp) = ui.allocate_exact_size(vec2(CUBE, FRAME_H), Sense::click());
        let p = ui.painter();
        shadowed(&p, fr, 14.0, if fresp.is_pointer_button_down_on() { RAISED() } else { tint(PANEL(), 0.88) }, shadow(false));
        p.rect_stroke(fr, 14.0, Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
        icons::paint(p, Rect::from_center_size(fr.center(), vec2(20.0, 20.0)), Icon::Frame, ink());
        if fresp.on_hover_text("Frame the selection (nothing selected: all)").clicked() {
            st.frame_current();
        }
    });
}

/// A navigation grip: a round button the tip drags, lit while it does.
fn nav_grip(ui: &mut Ui, id: &str, r: Rect, icon: Icon) -> egui::Response {
    let resp = ui.interact(r, Id::new(id), Sense::drag());
    let p = ui.painter();
    let on = resp.dragged() || resp.is_pointer_button_down_on();
    let fill = if on { tint(accent(), 0.22) } else if resp.hovered() { btn_bg() } else { Color32::TRANSPARENT };
    p.circle(r.center(), GRIP * 0.5 - 2.0, fill, Stroke::new(1.0, if on { accent() } else { Color32::TRANSPARENT }));
    icons::paint(p, Rect::from_center_size(r.center(), vec2(20.0, 20.0)), icon, if on { accent() } else { ink() });
    resp
}

/// A grip's drag this frame; nothing on the frame the tip comes down (the
/// Pencil comes down far from where it was last seen).
fn grip_moved(ui: &Ui, r: &egui::Response) -> egui::Vec2 {
    if ui.input(|i| i.pointer.any_pressed()) {
        egui::Vec2::ZERO
    } else {
        r.drag_delta()
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A point inside a convex polygon (either way round).
fn inside(q: &[egui::Pos2], p: egui::Pos2) -> bool {
    let mut sign = 0.0f32;
    for i in 0..q.len() {
        let a = q[i];
        let b = q[(i + 1) % q.len()];
        let c = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// The camera square to a face of the cube (Front, Right, Top or their
/// backs); the same face again: back to the perspective.
fn look_from(st: &mut AppState, n: glam::Vec3) {
    use std::f32::consts::{FRAC_PI_2, PI};
    st.stop_flight();
    let (yaw, pitch) = if n.z > 0.5 {
        (0.0, 0.0)
    } else if n.z < -0.5 {
        (PI, 0.0)
    } else if n.x > 0.5 {
        (FRAC_PI_2, 0.0)
    } else if n.x < -0.5 {
        (-FRAC_PI_2, 0.0)
    } else if n.y > 0.5 {
        (0.0, 1.52)
    } else {
        (0.0, -1.52)
    };
    let same = st.camera.ortho && (st.camera.yaw - yaw).abs() < 0.01 && (st.camera.pitch - pitch).abs() < 0.01;
    if same {
        st.camera.preset(ViewPreset::Perspective);
    } else {
        st.camera.yaw = yaw;
        st.camera.pitch = pitch;
        st.camera.ortho = true;
    }
}

/// The hint at the top of the view while a tool or a component mode is
/// on: the tool, then what the Pencil (or the mouse) does now.
fn hint_pill(ctx: &egui::Context, st: &mut AppState) {
    let busy = st.tool != Tool::Select || st.sel_mode != SelMode::Object || st.marking.is_open();
    if !st.settings.help || !busy || flag(ctx, "sheet") {
        return;
    }
    let parts: Vec<String> = if st.touch.on {
        super::tablet::touch_hint(st)
    } else {
        crate::viewport::hint(st).split(" · ").map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() <= 64).take(3).collect()
    };
    let vr = st.view_rect;
    egui::Area::new(Id::new("s-hint")).order(Order::Middle).interactable(false).pivot(Align2::CENTER_TOP).fixed_pos(pos2((vr[0] + vr[2]) * 0.5, TOP)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(24, 26, 30, 236))
            .corner_radius(egui::CornerRadius::same(20))
            .inner_margin(Margin::symmetric(16, 0))
            .shadow(shadow(false))
            .show(ui, |ui| {
                ui.set_height(40.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(egui::RichText::new(help_title(st)).font(semibold(13.5)).color(Color32::from_rgb(0xF4, 0xF5, 0xF7)));
                    for p in parts {
                        ui.label(egui::RichText::new("·").font(regular(13.5)).color(Color32::from_rgb(0x8E, 0x95, 0x9F)));
                        ui.label(egui::RichText::new(p).font(regular(13.5)).color(Color32::from_rgb(0xC9, 0xCD, 0xD3)));
                    }
                });
            });
    });
}

/// A size on the part (the mock-up's chips): a tap types it on the keypad.
/// The value typed, the frame after Done.
fn chip(ctx: &egui::Context, key: (&str, u32), at: egui::Pos2, text: &str, v: f32, lo: f32, hi: f32, lit: bool) -> Option<f32> {
    let mut out = None;
    egui::Area::new(Id::new(("s-chip", key.0, key.1))).order(Order::Middle).pivot(Align2::CENTER_CENTER).fixed_pos(at).show(ctx, |ui| {
        let g = ui.painter().layout_no_wrap(text.to_string(), semibold(13.0), if lit { accent_text() } else { TEXT_STRONG() });
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 30.0), Sense::click());
        if let Some(x) = ui.data_mut(|d| d.remove_temp::<f32>(resp.id.with("kp-result"))) {
            out = Some(x.clamp(lo, hi));
        }
        if resp.clicked() {
            let kp = Keypad { owner: resp.id, text: fmt3(v as f64), fresh: true, suffix: "mm".to_string(), lo, hi, anchor: r };
            ui.data_mut(|d| d.insert_temp(Id::new("keypad"), kp));
        }
        let typing = ui.data(|d| d.get_temp::<Keypad>(Id::new("keypad"))).is_some_and(|k| k.owner == resp.id);
        let p = ui.painter();
        shadowed(&p, r, 9.0, field_bg(), egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: pick(Color32::from_black_alpha(100), Color32::from_rgba_unmultiplied(20, 28, 40, 20)) });
        let line = if typing { Stroke::new(1.6, TEXT_STRONG()) } else if lit { Stroke::new(1.0, tint(accent(), 0.7)) } else { Stroke::new(1.0, field_line()) };
        p.rect_stroke(r, 9.0, line, StrokeKind::Inside);
        p.galley(r.center() - g.size() * 0.5, g, TEXT_STRONG());
    });
    out
}

/// Where a picked SDF shape meets the shapes added before it, as seen: of
/// its silhouette's points, the one nearest their surfaces (their own
/// distance fields; 22 nov.: the nearest pair of their outlines' points put
/// a ball's chip on a far corner of the block under it).
fn neck_of(st: &AppState, id: FeatureId, n: forme_core::NodeId, _own: &[glam::DVec3]) -> Option<glam::DVec3> {
    let f = st.doc.feature(id)?;
    let scene = crate::sdf::scene_of(f)?;
    let shapes = crate::sdf::built_shapes(f);
    let k = shapes.iter().position(|x| *x == n)?;
    let others: Vec<forme_core::Program> = shapes[..k].iter().filter(|o| scene.node(**o).map(|x| x.combine.mode == forme_core::CombineMode::Add).unwrap_or(false)).map(|o| forme_core::compile_single(scene, *o)).collect();
    if others.is_empty() {
        return None;
    }
    let m = crate::sdf::frame_matrix(f);
    let back = m.inverse();
    let mut best: Option<(f32, glam::Vec3)> = None;
    for line in crate::sdf::node_silhouette(f, scene, n, st.camera.eye()) {
        for p in line {
            let w = glam::Vec3::from(p);
            let q = back.transform_point3(w);
            let d = others.iter().map(|o| o.dist(q)).fold(f32::MAX, f32::min).abs();
            if best.map(|(b, _)| d < b).unwrap_or(true) {
                best = Some((d, w));
            }
        }
    }
    best.map(|(_, w)| w.as_dvec3())
}

/// The sizes of the selected Cube, Cylinder or Sketch on its edges, and
/// the blend of the picked SDF shape under it.
fn size_chips(ctx: &egui::Context, st: &mut AppState) {
    if st.tool != Tool::Select || st.drag.is_some() || st.creation.is_some() || st.marking.is_open() || st.align.is_some() {
        return;
    }
    let view = st.view();
    let vr = st.view_rect;
    let in_view = |p: egui::Pos2| p.x > vr[0] + 20.0 && p.x < vr[2] - 20.0 && p.y > vr[1] + 50.0 && p.y < vr[3] - 10.0;
    // an SDF shape picked: its blend under it
    if st.sdf_comp() {
        let Some((id, n)) = st.sdf_sel else { return };
        let Some(nd) = st.doc.feature(id).and_then(crate::sdf::scene_of).and_then(|s| s.node(n)).cloned() else { return };
        let pts = st.item_points(crate::common::Item::Shape(id, n));
        let sp: Vec<egui::Pos2> = pts.iter().filter_map(|q| view.project(q.as_vec3())).map(|s| pos2(s[0], s[1])).collect();
        if sp.is_empty() {
            return;
        }
        let cx = sp.iter().map(|q| q.x).sum::<f32>() / sp.len() as f32;
        let bottom = sp.iter().map(|q| q.y).fold(f32::MIN, f32::max);
        let b = nd.combine.blend;
        // the object's first shape blends with nothing: no chip
        let first = st.doc.feature(id).and_then(crate::sdf::scene_of).map(|s| crate::sdf::shapes(s).first() == Some(&n)).unwrap_or(true);
        if first {
            return;
        }
        // where it meets the shapes it is added to (the mock-up D3: the
        // chip at the neck, a dot on it), else under it
        let neck = (b > 0.0 && nd.combine.mode == forme_core::CombineMode::Add).then(|| neck_of(st, id, n, &pts)).flatten().and_then(|q| view.project(q.as_vec3())).map(|s| pos2(s[0], s[1])).filter(|q| in_view(*q));
        let at = match neck {
            Some(q) => {
                egui::Area::new(Id::new(("s-neck", id))).order(Order::Middle).interactable(false).pivot(Align2::CENTER_CENTER).fixed_pos(q).show(ctx, |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                    let p = ui.painter();
                    p.circle_filled(r.center(), 13.0, accent().gamma_multiply(0.22));
                    p.circle_filled(r.center(), 6.5, PANEL());
                    p.circle_stroke(r.center(), 6.5, Stroke::new(2.0, accent()));
                    p.circle_filled(r.center(), 2.2, accent());
                });
                pos2(q.x, q.y + 34.0)
            }
            None => pos2(cx, bottom + 30.0),
        };
        if !in_view(at) {
            return;
        }
        if let Some(x) = chip(ctx, ("blend", id), at, &format!("Blend {} mm", fmt3(b as f64)), b, 0.0, 30.0, true) {
            st.sdf_edit_node("Blend", |x2| x2.combine.blend = x);
        }
        return;
    }
    if st.sel_mode != SelMode::Object || st.selection.len() != 1 {
        return;
    }
    let Some(f) = st.single().cloned() else { return };
    if !plain_solid(&f) {
        return;
    }
    let world = |p: DVec3| f.origin + f.rot * p;
    let scr = |p: DVec3| view.project(world(p).as_vec3()).map(|s| pos2(s[0], s[1]));
    let (bx, by, bz, cyl) = match f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => (x, y, z, false),
        Prim::Cylinder { radius, height } => (radius * 2.0, height, radius * 2.0, true),
        Prim::Path { .. } => return,
    };
    let Some(centre) = scr(DVec3::new(0.0, by * 0.5, 0.0)) else { return };
    let away = |p: egui::Pos2| {
        let d = p - centre;
        let l = d.length().max(1.0);
        p + d / l * 26.0
    };
    // (the chip's key, the edge's middles to choose from, the value, the axis)
    let mut chips: Vec<(&str, egui::Pos2, f64, usize)> = vec![];
    let lowest = |cands: &[DVec3]| cands.iter().filter_map(|c| scr(*c)).max_by(|a, b| a.y.total_cmp(&b.y));
    let rightmost = |cands: &[DVec3]| cands.iter().filter_map(|c| scr(*c)).max_by(|a, b| a.x.total_cmp(&b.x));
    let (hx, hz) = (bx * 0.5, bz * 0.5);
    if cyl {
        let rim: Vec<DVec3> = (0..16).map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 16.0;
            DVec3::new(a.cos() * hx, 0.0, a.sin() * hz)
        }).collect();
        if let Some(p) = lowest(&rim) {
            chips.push(("d", away(p), bx, 0));
        }
        let side: Vec<DVec3> = rim.iter().map(|q| DVec3::new(q.x, by * 0.5, q.z)).collect();
        if let Some(p) = rightmost(&side) {
            chips.push(("h", away(p), by, 1));
        }
    } else {
        let wx: Vec<DVec3> = [(0.0, -hz), (0.0, hz), (by, -hz), (by, hz)].iter().map(|(y, z)| DVec3::new(0.0, *y, *z)).collect();
        let dz: Vec<DVec3> = [(-hx, 0.0), (hx, 0.0), (-hx, by), (hx, by)].iter().map(|(x, y)| DVec3::new(*x, *y, 0.0)).collect();
        let hy: Vec<DVec3> = [(-hx, -hz), (hx, -hz), (-hx, hz), (hx, hz)].iter().map(|(x, z)| DVec3::new(*x, by * 0.5, *z)).collect();
        if let Some(p) = lowest(&wx) {
            chips.push(("w", away(p), bx, 0));
        }
        if let Some(p) = lowest(&dz) {
            chips.push(("d", away(p), bz, 2));
        }
        if let Some(p) = rightmost(&hy) {
            chips.push(("h", away(p), by, 1));
        }
    }
    let id = f.id;
    for (k, at, v, axis) in chips {
        if !in_view(at) {
            continue;
        }
        let text = if cyl && axis == 0 { format!("Ø {} mm", fmt3(v)) } else { format!("{} mm", fmt3(v)) };
        if let Some(x) = chip(ctx, (k, id), at, &text, v as f32, 0.1, 2000.0, false) {
            let x = x.max(0.1) as f64;
            feature_edit(st, id, "Size", true, |g| match &mut g.prim {
                Prim::Box { x: a, y: b, z: c } | Prim::Profile { x: a, y: b, z: c } => *[a, b, c][axis] = x,
                Prim::Cylinder { radius, height } => {
                    if axis == 0 {
                        *radius = x * 0.5;
                    } else {
                        *height = x;
                    }
                }
                Prim::Path { .. } => {}
            });
            st.end_live();
        }
    }
}

/// One button of the pill: its icon and its word side by side (or the
/// icon alone).
fn pill_btn(ui: &mut Ui, icon: Icon, word: &str, color: Color32, on: bool) -> Response {
    let g = ui.painter().layout_no_wrap(word.to_string(), medium(13.0), color);
    let w = if word.is_empty() { 44.0 } else { g.size().x + 20.0 + 25.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 12.0, accent_bg());
    } else if resp.is_pointer_button_down_on() || resp.hovered() {
        p.rect_filled(r, 12.0, RAISED());
    }
    let c = if on { accent_text() } else { color };
    if word.is_empty() {
        icons::paint(p, Rect::from_center_size(r.center(), vec2(18.0, 18.0)), icon, c);
    } else {
        icons::paint(p, Rect::from_center_size(pos2(r.min.x + 19.0, r.center().y), vec2(18.0, 18.0)), icon, c);
        p.galley(pos2(r.min.x + 35.0, r.center().y - g.size().y * 0.5), g, c);
    }
    resp
}

/// The actions over what is selected or picked (the mock-up's pill): the
/// gizmo (move, turn, size), Copy, Mirror, Repeat, Hollow, Delete; picked
/// edges' Round, Chamfer, Loop; picked faces' Push, Inset, Hollow.
fn actions_pill(ctx: &egui::Context, st: &mut AppState) {
    let Some(at) = super::tablet::bar_at(st) else { return };
    ctx.data_mut(|d| d.remove::<Rect>(Id::new("t-actions-rect")));
    egui::Area::new(Id::new("s-pill")).order(Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        pop_glass(16).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let count = |ui: &mut Ui, text: String| {
                    let g = ui.painter().layout_no_wrap(text, semibold(13.0), TEXT2());
                    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 44.0), Sense::hover());
                    ui.painter().galley(pos2(r.min.x + 10.0, r.center().y - g.size().y * 0.5), g, TEXT2());
                };
                let gizmo = |ui: &mut Ui, st: &mut AppState| {
                    for (g, icon, tip) in [(GizmoMode::Move, Icon::Move, "Move"), (GizmoMode::Rotate, Icon::Rotate, "Turn"), (GizmoMode::Scale, Icon::Scale, "Size")] {
                        if pill_btn(ui, icon, "", TEXT(), st.gizmo == g).on_hover_text(tip).clicked() {
                            st.set_gizmo(if st.gizmo == g { GizmoMode::Select } else { g });
                        }
                    }
                    bar_sep(ui, 44.0);
                };
                if st.selection.is_empty() {
                    let deselect = |ui: &mut Ui, st: &mut AppState| {
                        bar_sep(ui, 44.0);
                        if pill_btn(ui, Icon::Close, "", TEXT(), false).on_hover_text("Deselect").clicked() {
                            st.edge_sel.clear();
                            st.face_sel.clear();
                            st.vert_sel.clear();
                        }
                    };
                    if !st.edge_sel.is_empty() {
                        let n = st.edge_sel.len();
                        count(ui, format!("{n} edge{}", if n > 1 { "s" } else { "" }));
                        for (c, word) in [(false, "Round"), (true, "Chamfer")] {
                            if pill_btn(ui, Icon::Fillet, word, TEXT(), false).clicked() {
                                st.round_chamfer = c;
                                st.set_round(1.0, c, true);
                                st.end_live();
                            }
                        }
                        if pill_btn(ui, Icon::Loop, "Loop", TEXT(), false).clicked() {
                            if let Some(i) = st.edge_sel.first().and_then(|r| st.model_edge_of(r)) {
                                st.pick_linked_edges(i, true);
                            }
                        }
                        deselect(ui, st);
                    } else if !st.face_sel.is_empty() {
                        let n = st.face_sel.len();
                        count(ui, format!("{n} face{}", if n > 1 { "s" } else { "" }));
                        if pill_btn(ui, Icon::Extrude, "Push", TEXT(), false).clicked() {
                            st.arm_push(PushKind::Extrude, None);
                        }
                        if pill_btn(ui, Icon::Inset, "Inset", TEXT(), false).clicked() {
                            st.arm_push(PushKind::Inset, None);
                        }
                        if pill_btn(ui, Icon::Shell, "Hollow", TEXT(), false).clicked() {
                            st.add_shell();
                        }
                        deselect(ui, st);
                    } else {
                        let n = st.vert_sel.len();
                        count(ui, format!("{n} point{}", if n > 1 { "s" } else { "" }));
                        if pill_btn(ui, Icon::Frame, "Frame", TEXT(), false).clicked() {
                            st.frame_current();
                        }
                        deselect(ui, st);
                    }
                    return;
                }
                // an SDF shape picked in its object
                if st.sdf_comp() && st.sdf_picked().is_some() {
                    gizmo(ui, st);
                    let nd = st.sdf_picked().and_then(|(f, n)| crate::sdf::scene_of(f).and_then(|s| s.node(n)).cloned());
                    if pill_btn(ui, Icon::Duplicate, "Copy", TEXT(), false).clicked() {
                        st.sdf_duplicate();
                    }
                    let mx = nd.as_ref().map(|n| n.mirror.x).unwrap_or(false);
                    if pill_btn(ui, Icon::Mirror, "Mirror", TEXT(), mx).clicked() {
                        st.sdf_edit_node("Mirror X", |x| x.mirror.x = !mx);
                    }
                    let rep = nd.as_ref().map(|n| !matches!(n.repeat, forme_core::Repeat::None)).unwrap_or(false);
                    if pill_btn(ui, Icon::Array, "Repeat", TEXT(), rep).clicked() {
                        let r = if rep { forme_core::Repeat::None } else { forme_core::Repeat::Radial { count: 6, axis: forme_core::Axis::Y } };
                        st.sdf_edit_node("Repeat", |x| x.repeat = r);
                    }
                    bar_sep(ui, 44.0);
                    if pill_btn(ui, Icon::Trash, "", carve_light(), false).on_hover_text("Delete").clicked() {
                        st.sdf_delete();
                    }
                    return;
                }
                let solids: Vec<FeatureId> = st.selection.iter().copied().filter(|id| st.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
                // Mirror, Repeat and Hollow are steps of the Crisp part: not
                // for a mesh (a Soft object picked whole)
                let part = solids.iter().all(|id| st.doc.feature(*id).map(|f| !f.is_mesh()).unwrap_or(false));
                if !solids.is_empty() {
                    gizmo(ui, st);
                    if pill_btn(ui, Icon::Duplicate, "Copy", TEXT(), false).clicked() {
                        st.duplicate_selected();
                    }
                    if st.workspace == Workspace::Forme && part {
                        if pill_btn(ui, Icon::Mirror, "Mirror", TEXT(), false).clicked() {
                            st.add_mirror(0);
                        }
                        if pill_btn(ui, Icon::Array, "Repeat", TEXT(), false).clicked() {
                            st.add_array();
                        }
                        if pill_btn(ui, Icon::Shell, "Hollow", TEXT(), false).clicked() {
                            st.add_shell();
                        }
                    }
                    if st.can_group() && st.selection.len() > 1 && pill_btn(ui, Icon::Group, "Group", TEXT(), false).clicked() {
                        st.group_selection();
                    }
                } else if pill_btn(ui, Icon::Frame, "Frame", TEXT(), false).clicked() {
                    st.frame_selection();
                }
                bar_sep(ui, 44.0);
                if pill_btn(ui, Icon::Trash, "", carve_light(), false).on_hover_text("Delete").clicked() {
                    st.delete_selected();
                }
            });
        });
    });
}
