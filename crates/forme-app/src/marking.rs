//! Marking menu (Maya / WZX Engine style): a radial menu of 8 directions
//! around the pointer, with items that depend on what is under it (a shape,
//! an edge in Arête mode, or empty space).
//!
//! - Right button: the menu opens on the press. A quick flick in a direction
//!   and release runs that item without drawing the menu (the item then
//!   flashes alone so the choice is seen); held, the menu is drawn after
//!   0.2 s and the release on an item runs it.
//! - Left button held still ~0.28 s (Select tool, not on a handle): the menu
//!   opens, drawn at once. Moving before that keeps the usual gestures
//!   (orbit in empty space, handles…).
//! - On a shape, the top item opens the material arc (BrickIt style): the
//!   swatches come out one by one, release on one to apply it.
//!
//! The items are listed in `items()`: change them there.

use crate::camera::ViewPreset;
use crate::state::{AppState, PickMode, Tool};
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, Id, LayerId, Order, Painter, Pos2, Rect, Stroke};
use forme_core::{CombineMode, NodeId, NodeKind, PrimKind};

/// Left button held this long without moving opens the menu (seconds).
pub const HOLD: f64 = 0.28;
/// Movement (points) that turns a left press back into a normal gesture.
const HOLD_SLOP: f32 = 5.0;
/// Right press: the menu is drawn after this delay (a flick runs before it).
const DELAY: f64 = 0.2;
/// The chosen item stays lit this long after a flick.
const FLASH: f64 = 0.5;
/// Distance of the items from the centre (points), dead zone around it.
const RADIUS: f32 = 84.0;
const DEAD: f32 = 20.0;
/// Material arc: radius from the centre, swatch radius, spread (radians).
const ARC_R: f32 = 150.0;
const SWATCH: f32 = 11.0;
const ARC_SPREAD: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ctx {
    Shape(NodeId),
    Edge,
    Empty,
}

impl Ctx {
    fn title(self) -> &'static str {
        match self {
            Ctx::Shape(_) => "Forme",
            Ctx::Edge => "Arête",
            Ctx::Empty => "Scène",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// Opens the material arc (no action by itself).
    Material,
    Duplicate,
    MirrorX,
    ToggleCarve,
    Frame,
    Delete,
    Hide,
    Group,
    EdgeMode,
    ObjectMode,
    Place(PrimKind),
    Snap,
    Grid,
    View(ViewPreset),
    Bake,
    Fillet,
    Chamfer,
    Sharp,
}

pub struct Item {
    /// 0 N, 1 NE, 2 E, 3 SE, 4 S, 5 SW, 6 W, 7 NW.
    pub dir: u8,
    pub label: String,
    pub checked: bool,
    pub act: Act,
}

fn it(dir: u8, label: &str, checked: bool, act: Act) -> Item {
    Item { dir, label: label.to_string(), checked, act }
}

/// The items of each context. Change the menu here.
pub fn items(st: &AppState, ctx: Ctx) -> Vec<Item> {
    match ctx {
        Ctx::Shape(id) => {
            let n = st.scene.node(id);
            let carve = n.map(|n| n.combine.mode == CombineMode::Carve).unwrap_or(false);
            let mirror = n.map(|n| n.mirror.x).unwrap_or(false);
            vec![
                it(0, "Matière ›", false, Act::Material),
                it(1, "Symétrie X", mirror, Act::MirrorX),
                it(2, if carve { "Passer en Add (+)" } else { "Passer en Carve (−)" }, false, Act::ToggleCarve),
                it(3, "Cadrer", false, Act::Frame),
                it(4, "Supprimer", false, Act::Delete),
                it(5, "Masquer", false, Act::Hide),
                it(6, "Mode Arête", false, Act::EdgeMode),
                it(7, "Dupliquer", false, Act::Duplicate),
            ]
        }
        Ctx::Edge => {
            let e = st.edge_sel.last();
            let (r, ch) = e.map(|e| (e.radius, e.chamfer)).unwrap_or((0.0, false));
            vec![
                it(0, "Fillet", r > 0.0 && !ch, Act::Fillet),
                it(2, "Chanfrein", r > 0.0 && ch, Act::Chamfer),
                it(3, "Cadrer", false, Act::Frame),
                it(4, "Arête vive", r <= 0.0, Act::Sharp),
                it(6, "Mode Objet", false, Act::ObjectMode),
            ]
        }
        Ctx::Empty => {
            let preset = st.camera.preset_of();
            let ortho = st.camera.ortho;
            vec![
                it(0, "Cube", st.tool == Tool::Place(PrimKind::Box), Act::Place(PrimKind::Box)),
                it(1, "Cylindre", st.tool == Tool::Place(PrimKind::Cylinder), Act::Place(PrimKind::Cylinder)),
                it(2, "Snap", st.snap, Act::Snap),
                it(3, "Grille", st.grid, Act::Grid),
                it(4, "Vue 3D", !ortho, Act::View(ViewPreset::Perspective)),
                it(5, "Vue de dessus", ortho && preset == ViewPreset::Top, Act::View(ViewPreset::Top)),
                it(6, "Vue de face", ortho && preset == ViewPreset::Front, Act::View(ViewPreset::Front)),
                it(7, "Grouper", false, Act::Group),
            ]
        }
    }
}

pub fn run(st: &mut AppState, act: Act, ctx: Ctx) {
    match act {
        Act::Material => {}
        Act::Duplicate => st.duplicate_selected(),
        Act::MirrorX => {
            if let Ctx::Shape(id) = ctx {
                let v = !st.scene.node(id).map(|n| n.mirror.x).unwrap_or(false);
                st.edit_selection("Symétrie", id, |n| n.mirror.x = v);
            }
        }
        Act::ToggleCarve => {
            if let Ctx::Shape(id) = ctx {
                let m = match st.scene.node(id).map(|n| n.combine.mode) {
                    Some(CombineMode::Carve) => CombineMode::Add,
                    _ => CombineMode::Carve,
                };
                st.edit_selection("Mode", id, |n| n.combine.mode = m);
            }
        }
        Act::Frame => st.frame_selection(),
        Act::Delete => st.delete_selected(),
        Act::Hide => {
            if let Ctx::Shape(id) = ctx {
                let v = !st.scene.node(id).map(|n| n.visible).unwrap_or(true);
                st.edit_selection("Visibilité", id, |n| n.visible = v);
            }
        }
        Act::Group => st.group_selected(),
        Act::EdgeMode => st.set_pick_mode(PickMode::Edge),
        Act::ObjectMode => st.set_pick_mode(PickMode::Object),
        Act::Place(k) => {
            st.commit_placement();
            st.tool = Tool::Place(k);
        }
        Act::Snap => st.snap = !st.snap,
        Act::Grid => st.grid = !st.grid,
        Act::View(p) => st.camera.preset(p),
        Act::Bake => {} // the Bake is set aside
        Act::Fillet | Act::Chamfer => {
            let r = st.edge_sel.last().map(|e| e.radius).unwrap_or(0.0);
            st.set_fillet(if r > 0.0 { r } else { 1.0 }, act == Act::Chamfer, true);
        }
        Act::Sharp => {
            let ch = st.edge_sel.last().map(|e| e.chamfer).unwrap_or(false);
            st.set_fillet(0.0, ch, true);
        }
    }
    st.view_changed();
}

/// Open menu.
pub struct Menu {
    pub center: [f32; 2],
    pub pointer: [f32; 2],
    pub hover: Option<u8>,
    pub ctx: Ctx,
    /// Opened by the right button (else a left press held still).
    pub right: bool,
    pub t0: f64,
    pub shown: bool,
    /// Material arc: when it opened, and the swatch under the pointer.
    pub arc: Option<f64>,
    pub swatch: Option<usize>,
    /// Opened by a right click (Tripo3D): stays open, the next click picks.
    pub sticky: bool,
}

impl Menu {
    pub fn new(center: [f32; 2], ctx: Ctx, right: bool, now: f64) -> Menu {
        Menu { center, pointer: center, hover: None, ctx, right, t0: now, shown: !right, arc: None, swatch: None, sticky: false }
    }
}

/// After a flick: the chosen item alone, fading out.
pub struct Flash {
    pub center: [f32; 2],
    pub dir: u8,
    pub label: String,
    pub t: f64,
}

#[derive(Default)]
pub struct Marking {
    pub menu: Option<Menu>,
    pub flash: Option<Flash>,
    /// Left press waiting to become a menu: where and when.
    press: Option<([f32; 2], f64)>,
    /// Tripo3D: right press waiting to become a menu (click or held still)
    /// or a pan (moved).
    rpress: Option<([f32; 2], f64)>,
    /// A sticky menu closed by a press: that press is not passed on.
    swallow: bool,
    /// Captures: the open menu stays frozen on screen.
    pub demo: bool,
}

impl Marking {
    pub fn is_open(&self) -> bool {
        self.menu.is_some()
    }
    /// Tripo3D: a right press that has not moved yet (no pan meanwhile).
    pub fn right_pending(&self) -> bool {
        self.rpress.is_some()
    }
}

fn dir_of(dx: f32, dy: f32) -> u8 {
    let a = dx.atan2(-dy); // 0 = up, clockwise
    let d = (a / (std::f32::consts::PI / 4.0)).round() as i32;
    ((d + 8) % 8) as u8
}

/// Centre of swatch `k` of `n` on the material arc (above the menu).
fn swatch_at(c: Pos2, k: usize, n: usize) -> Pos2 {
    let t = if n <= 1 { 0.5 } else { k as f32 / (n - 1) as f32 };
    let spread = ARC_SPREAD.min(0.32 * n as f32);
    let a = (t - 0.5) * spread;
    pos2(c.x + a.sin() * ARC_R, c.y - a.cos() * ARC_R * 0.86)
}

/// What is under the pointer; selects it (shape, or edge in Arête mode).
fn context_at(st: &mut AppState, pos: [f32; 2]) -> Ctx {
    if st.pick_mode == PickMode::Edge {
        if let Some(e) = st.edge_hover.clone().or_else(|| st.pick_edge(pos, 12.0)) {
            if !st.edge_selected(&e) {
                st.select_edge(e, false);
            }
            return Ctx::Edge;
        }
    }
    match st.node_at(pos) {
        Some(id) => {
            if !st.is_selected(id) {
                st.select(Some(id));
            } else if st.selection != Some(id) {
                // part of a multiple selection: it becomes the active one
                st.selection_more.retain(|x| *x != id);
                if let Some(old) = st.selection {
                    st.selection_more.push(old);
                }
                st.selection = Some(id);
            }
            match st.scene.node(id).map(|n| &n.kind) {
                Some(NodeKind::Shape(_)) | Some(NodeKind::Group { .. }) => Ctx::Shape(id),
                _ => Ctx::Empty,
            }
        }
        None => Ctx::Empty,
    }
}

/// Viewport input. `on_handle`: the pointer is on a selection handle (a left
/// hold there is a handle drag, never the menu). Returns true when the menu
/// takes the pointer this frame (the viewport then ignores it).
pub fn input(ctx: &egui::Context, st: &mut AppState, rect: Rect, on_handle: bool) -> bool {
    if st.marking.demo {
        return st.marking.is_open();
    }
    if cfg!(target_os = "ios") || st.placement.is_some() {
        st.marking.press = None;
        return st.marking.is_open();
    }
    let now = st.now;
    let (pos, sec_pressed, sec_down, pri_pressed, pri_down) = ctx.input(|i| {
        (
            i.pointer.interact_pos().or(i.pointer.hover_pos()),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.button_down(egui::PointerButton::Secondary),
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_down(egui::PointerButton::Primary),
        )
    });
    let Some(pos) = pos else { return st.marking.is_open() };
    let p = [pos.x, pos.y];
    // only over the 3D view itself (not over a panel)
    let in_view = rect.contains(pos) && ctx.layer_id_at(pos).map(|l| l.order == Order::Background).unwrap_or(false);

    // the press that closed a sticky menu goes nowhere else
    if st.marking.swallow {
        if pri_down || sec_down {
            return true;
        }
        st.marking.swallow = false;
    }

    if st.marking.menu.is_none() {
        let maya = st.settings.navigation == crate::settings::Navigation::Maya;
        // Maya: right press, the menu at once (drawn after the delay, flicks allowed)
        if sec_pressed && in_view && maya {
            st.marking.press = None;
            let c = context_at(st, p);
            st.marking.menu = Some(Menu::new(p, c, true, now));
            ctx.request_repaint();
            return true;
        }
        // Tripo3D: the right drag pans, so the menu waits: a right click
        // (released without moving) opens it until the next click, a right
        // press held still opens it like in Maya (release on an item)
        if !maya {
            if sec_pressed && in_view {
                st.marking.rpress = Some((p, now));
            }
            if let Some((p0, t)) = st.marking.rpress {
                if (p[0] - p0[0]).hypot(p[1] - p0[1]) > HOLD_SLOP {
                    st.marking.rpress = None;
                } else if !sec_down {
                    st.marking.rpress = None;
                    let c = context_at(st, p0);
                    let mut m = Menu::new(p0, c, true, now);
                    m.sticky = true;
                    m.shown = true;
                    st.marking.menu = Some(m);
                    ctx.request_repaint();
                    return true;
                } else if now - t >= HOLD {
                    st.marking.rpress = None;
                    let c = context_at(st, p0);
                    let mut m = Menu::new(p0, c, true, now);
                    m.shown = true;
                    st.marking.menu = Some(m);
                    ctx.request_repaint();
                    return true;
                } else {
                    ctx.request_repaint();
                }
            }
        }
        // left press held still: the menu (Select tool, not on a handle)
        if pri_pressed && in_view && st.tool == Tool::Select && !on_handle && !ctx.input(|i| i.modifiers.shift) {
            st.marking.press = Some((p, now));
        }
        if let Some((p0, t)) = st.marking.press {
            if !pri_down || (p[0] - p0[0]).hypot(p[1] - p0[1]) > HOLD_SLOP {
                st.marking.press = None;
            } else if now - t >= HOLD {
                st.marking.press = None;
                let c = context_at(st, p0);
                st.marking.menu = Some(Menu::new(p0, c, false, now));
                ctx.request_repaint();
                return true;
            } else {
                ctx.request_repaint();
            }
        }
        if st.marking.flash.as_ref().map(|f| now - f.t < FLASH).unwrap_or(false) {
            ctx.request_repaint();
        }
        return false;
    }

    // menu open: follow the pointer, run the item on release
    let n_mats = st.scene.materials.len();
    let menu = st.marking.menu.as_mut().unwrap();
    menu.pointer = p;
    let c = pos2(menu.center[0], menu.center[1]);
    let (dx, dy) = (p[0] - c.x, p[1] - c.y);
    let dist = dx.hypot(dy);
    menu.hover = if dist > DEAD { Some(dir_of(dx, dy)) } else { None };
    if !menu.shown && now - menu.t0 >= DELAY {
        menu.shown = true;
    }
    // the material arc: opens when the pointer reaches the top item of a shape
    if matches!(menu.ctx, Ctx::Shape(_)) {
        if menu.arc.is_none() && menu.hover == Some(0) && dist > RADIUS * 0.7 {
            menu.arc = Some(now);
            menu.shown = true;
        }
        if menu.arc.is_some() && dist < DEAD {
            menu.arc = None;
        }
        menu.swatch = None;
        if menu.arc.is_some() {
            let mut best = (SWATCH + 8.0, None);
            for k in 0..n_mats {
                let d = swatch_at(c, k, n_mats).distance(pos);
                if d < best.0 {
                    best = (d, Some(k));
                }
            }
            menu.swatch = best.1;
        }
    }
    let sticky = menu.sticky;
    let released = if sticky { pri_pressed || sec_pressed } else if menu.right { !sec_down } else { !pri_down };
    if sticky && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.marking.menu = None;
        return true;
    }
    if released {
        if sticky {
            st.marking.swallow = true;
        }
        let menu = st.marking.menu.take().unwrap();
        if let (Some(k), Ctx::Shape(id)) = (menu.swatch, menu.ctx) {
            let name = st.scene.materials.get(k).map(|m| m.name.clone()).unwrap_or_default();
            st.edit_selection("Matière", id, |n| n.material = k as u32);
            st.toast(format!("Matière : {name}"));
        } else if let Some(d) = menu.hover {
            let list = items(st, menu.ctx);
            if let Some(item) = list.into_iter().find(|i| i.dir == d && i.act != Act::Material) {
                if !menu.shown {
                    st.marking.flash = Some(Flash { center: menu.center, dir: d, label: item.label.clone(), t: now });
                }
                run(st, item.act, menu.ctx);
                st.toast(format!("Menu : {}", item.label));
            }
        }
    }
    ctx.request_repaint();
    true
}

/// Rectangle of an item's pill: the side items grow away from the centre.
fn pill_rect(p: &Painter, c: Pos2, dir: u8, label: &str) -> (Rect, std::sync::Arc<egui::Galley>) {
    let a = dir as f32 * std::f32::consts::PI / 4.0;
    let at = pos2(c.x + a.sin() * RADIUS, c.y - a.cos() * RADIUS * 0.82);
    let g = p.layout_no_wrap(label.to_string(), medium(12.0), TEXT);
    let (pw, ph) = (g.size().x + 18.0, g.size().y + 8.0);
    let x0 = if a.sin() > 0.3 {
        at.x - 8.0
    } else if a.sin() < -0.3 {
        at.x - pw + 8.0
    } else {
        at.x - pw * 0.5
    };
    (Rect::from_min_size(pos2(x0, at.y - ph * 0.5), vec2(pw, ph)), g)
}

const HOT: Color32 = Color32::from_rgb(255, 226, 102);

fn draw_pill(p: &Painter, r: Rect, g: std::sync::Arc<egui::Galley>, hot: bool, alpha: f32) {
    let f = |c: Color32| c.gamma_multiply(alpha);
    let ph = r.height();
    p.rect(
        r,
        ph * 0.5,
        f(if hot { HOT } else { Color32::from_rgba_unmultiplied(34, 36, 42, 236) }),
        Stroke::new(1.0, f(if hot { HOT } else { Color32::from_rgba_unmultiplied(110, 114, 124, 200) })),
        egui::StrokeKind::Inside,
    );
    let col = if hot { Color32::from_rgb(28, 28, 30) } else { Color32::from_rgb(228, 228, 232) };
    p.galley_with_override_text_color(pos2(r.min.x + 9.0, r.center().y - g.size().y * 0.5), g, f(col));
}

/// Établi's radial menu: eight sectors of a ring around the press point,
/// labels inside, the hovered sector filled with the accent.
fn draw_pie(p: &Painter, st: &AppState, m: &Menu, c: Pos2) {
    use std::f32::consts::PI;
    let (r0, r1) = (DEAD + 8.0, RADIUS + 34.0);
    let acc = crate::theme::accent();
    let list = items(st, m.ctx);
    // the disc behind, so the scene does not show through the gaps
    p.circle_filled(c, r1 + 3.0, Color32::from_rgba_unmultiplied(14, 16, 18, 150));
    for dir in 0u8..8 {
        let a = dir as f32 * PI / 4.0;
        let (a0, a1) = (a - PI / 8.0 + 0.025, a + PI / 8.0 - 0.025);
        let n = 10;
        let mut pts = vec![];
        for i in 0..=n {
            let t = a0 + (a1 - a0) * i as f32 / n as f32;
            pts.push(c + vec2(t.sin(), -t.cos()) * r1);
        }
        for i in (0..=n).rev() {
            let t = a0 + (a1 - a0) * i as f32 / n as f32;
            pts.push(c + vec2(t.sin(), -t.cos()) * r0);
        }
        let item = list.iter().find(|i| i.dir == dir);
        let hot = item.is_some() && m.hover == Some(dir) && m.swatch.is_none() || (item.map(|i| i.act == Act::Material).unwrap_or(false) && m.arc.is_some());
        // opaque fills: the slices overlap a hair at their seams
        let mix = |k: f32| Color32::from_rgb((28.0 + (acc.r() as f32 - 28.0) * k) as u8, (31.0 + (acc.g() as f32 - 31.0) * k) as u8, (35.0 + (acc.b() as f32 - 35.0) * k) as u8);
        let fill = if hot { mix(0.32) } else { Color32::from_rgb(28, 31, 35) };
        let stroke = if hot { Stroke::new(1.5, acc) } else { Stroke::new(1.0, Color32::from_rgba_unmultiplied(62, 68, 76, 255)) };
        // a ring sector is not convex: filled as thin convex slices
        let m1 = n + 1;
        for i in 0..n {
            let quad = vec![pts[i], pts[i + 1], pts[2 * m1 - 2 - i], pts[2 * m1 - 1 - i]];
            p.add(egui::Shape::convex_polygon(quad, fill, egui::Stroke::NONE));
        }
        p.add(egui::Shape::closed_line(pts, stroke));
        if let Some(it) = item {
            let mid = c + vec2(a.sin(), -a.cos()) * ((r0 + r1) * 0.5);
            let label = if it.checked { format!("• {}", it.label) } else { it.label.clone() };
            let g = p.layout(label, regular(10.5), if hot { acc } else { TEXT }, 62.0);
            p.galley(mid - g.size() * 0.5, g, TEXT);
        }
    }
    p.circle_filled(c, r0 - 6.0, Color32::from_rgba_unmultiplied(20, 22, 25, 240));
    p.circle_stroke(c, r0 - 6.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(62, 68, 76, 255)));
    p.circle_filled(c, 3.0, acc);
    p.text(c + vec2(0.0, r1 + 8.0), Align2::CENTER_TOP, m.ctx.title(), medium(10.5), Color32::from_rgba_unmultiplied(236, 236, 226, 170));
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

pub fn draw(ctx: &egui::Context, st: &AppState) {
    let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("marking-menu")));
    let now = st.now;
    if let Some(f) = &st.marking.flash {
        let t = (now - f.t) as f32;
        if t >= 0.0 && t < FLASH as f32 {
            let alpha = 1.0 - t / FLASH as f32;
            let c = pos2(f.center[0], f.center[1]);
            let (r, g) = pill_rect(&p, c, f.dir, &f.label);
            p.line_segment([c, r.center()], Stroke::new(1.2, HOT.gamma_multiply(alpha * 0.8)));
            p.circle_filled(c, 4.0, Color32::from_rgb(236, 236, 226).gamma_multiply(alpha * 0.8));
            draw_pill(&p, r, g, true, alpha);
        }
    }
    let Some(m) = &st.marking.menu else { return };
    if !m.shown {
        return;
    }
    let c = pos2(m.center[0], m.center[1]);
    let pointer = pos2(m.pointer[0], m.pointer[1]);
    if st.settings.ui_mode != crate::settings::UiMode::Studio {
        // Établi and Focus: a radial pie at the cursor, the item under the pointer lit green
        draw_pie(&p, st, m, c);
    } else {
    p.circle_filled(c, 4.5, Color32::from_rgba_unmultiplied(236, 236, 226, 200));
    p.circle_stroke(c, DEAD, Stroke::new(1.0, Color32::from_rgba_unmultiplied(236, 236, 226, 70)));
    if m.hover.is_some() {
        p.line_segment([c, pointer], Stroke::new(1.2, HOT.gamma_multiply(0.8)));
    }
    p.text(c + vec2(0.0, DEAD + 6.0), Align2::CENTER_TOP, m.ctx.title(), regular(11.0), Color32::from_rgba_unmultiplied(236, 236, 226, 160));
    for item in items(st, m.ctx) {
        let label = if item.checked { format!("• {}", item.label) } else { item.label.clone() };
        let (r, g) = pill_rect(&p, c, item.dir, &label);
        let hot = m.hover == Some(item.dir) && m.swatch.is_none();
        draw_pill(&p, r, g, hot || (item.act == Act::Material && m.arc.is_some()), 1.0);
    }
    }
    // material arc: the swatches come out one by one
    if let (Some(t0), Ctx::Shape(id)) = (m.arc, m.ctx) {
        let cur = st.scene.node(id).map(|n| n.material as usize);
        let mats = &st.scene.materials;
        let n = mats.len();
        for (k, mat) in mats.iter().enumerate() {
            let t = if st.marking.demo { 1.0 } else { ((now - t0 - k as f64 * 0.0125) / 0.09) as f32 };
            let e = ease_out(t);
            if e <= 0.0 {
                continue;
            }
            let at = swatch_at(c, k, n);
            let from = pos2(c.x, c.y - RADIUS * 0.82);
            let q = from + (at - from) * e;
            let hot = m.swatch == Some(k);
            let r = SWATCH * e * if hot { 1.3 } else { 1.0 };
            let col = Color32::from_rgb((mat.color[0] * 255.0) as u8, (mat.color[1] * 255.0) as u8, (mat.color[2] * 255.0) as u8);
            p.circle_filled(q + vec2(0.0, 1.0), r + 1.5, Color32::from_black_alpha((90.0 * e) as u8));
            p.circle_filled(q, r, col.gamma_multiply(e));
            let ring = if hot { HOT } else if cur == Some(k) { Color32::WHITE } else { Color32::from_rgba_unmultiplied(110, 114, 124, 200) };
            p.circle_stroke(q, r, Stroke::new(if hot || cur == Some(k) { 2.0 } else { 1.0 }, ring.gamma_multiply(e)));
            if hot {
                p.text(q + vec2(0.0, -r - 6.0), Align2::CENTER_BOTTOM, &mat.name, medium(12.0), TEXT);
            }
        }
    }
}
