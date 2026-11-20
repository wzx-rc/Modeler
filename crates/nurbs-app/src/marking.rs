//! Marking menu (Maya / WZX Engine), light: a few items in 8 directions
//! round the pointer, by what is under it (a shape, a picked edge or face,
//! empty space). Taken from the SDF version's menu (branch `claude/nurbs`),
//! its look and gestures, not its items.
//!
//! - Right button pressed in the view (no Alt): the menu opens at once. A
//!   quick flick and release runs an item without drawing the menu (the item
//!   flashes alone so the choice is seen); held, the menu is drawn after
//!   0.2 s and the release on an item runs it. Released in the middle:
//!   nothing.
//! - On a shape, the top item opens the material arc (BrickIt style): the
//!   six balls come out one by one, release on one to give it.
//! - Not while a gesture is live, nor with the Knife (its right click ends
//!   the cut).
//!
//! The items are listed in `items()`: change them there.

use crate::materials::MATERIALS;
use crate::state::{AppState, SelMode, Tool};
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, Id, LayerId, Order, Painter, Pos2, Rect, Stroke};
use nurbs_kernel::doc::PrimKind;
use nurbs_kernel::{FeatureId, Op};

/// The menu is drawn after this delay (a flick runs before it), seconds.
const DELAY: f64 = 0.2;
/// The chosen item stays lit this long after a flick.
const FLASH: f64 = 0.5;
/// Distance of the items from the centre (points), dead zone round it.
const RADIUS: f32 = 84.0;
const DEAD: f32 = 20.0;
/// Material arc: radius from the centre, ball radius, spread (radians).
const ARC_R: f32 = 150.0;
const BALL: f32 = 12.0;
const ARC_SPREAD: f32 = 1.7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ctx {
    Shape(FeatureId),
    /// A fillet, chamfer, inset, move… (an operation, no shape of its own).
    Step(FeatureId),
    Edge,
    Face,
    Empty,
}

impl Ctx {
    fn title(self) -> &'static str {
        match self {
            Ctx::Shape(_) => "Shape",
            Ctx::Step(_) => "Operation",
            Ctx::Edge => "Edges",
            Ctx::Face => "Faces",
            Ctx::Empty => "Scene",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// Opens the material arc (no action by itself).
    Material,
    ToggleCarve,
    Frame,
    FrameAll,
    Delete,
    Hide,
    Duplicate,
    Fillet,
    Chamfer,
    /// The fillet at the radius read on the reference mesh.
    FitScan,
    Extrude,
    Inset,
    Deselect,
    ObjectMode,
    /// A component mode (Vertex, Edge, Face, Object).
    Mode(SelMode),
    Place(PrimKind),
    Snap,
    /// An operation turned off (its element's history keeps it).
    TurnOff,
    /// The element's history marker put just after this operation.
    Marker,
}

pub struct Item {
    /// 0 N, 1 NE, 2 E, 3 SE, 4 S, 5 SW, 6 W, 7 NW.
    pub dir: u8,
    pub label: &'static str,
    pub checked: bool,
    pub act: Act,
}

fn it(dir: u8, label: &'static str, checked: bool, act: Act) -> Item {
    Item { dir, label, checked, act }
}

/// The items of each context. Change the menu here.
pub fn items(st: &AppState, ctx: Ctx) -> Vec<Item> {
    match ctx {
        // on the part, only the four modes (Romain, 20 nov. 5 h 52: "ne
        // mettre que les 4 états, Object, Edge, Point, Face"), placed as
        // in Maya: Vertex west, Edge east, Face south, Object NE
        Ctx::Shape(_) | Ctx::Step(_) | Ctx::Edge | Ctx::Face => {
            let m = st.sel_mode;
            vec![
                it(1, "Object", m == SelMode::Object, Act::Mode(SelMode::Object)),
                it(2, "Edge", m == SelMode::Edge, Act::Mode(SelMode::Edge)),
                it(4, "Face", m == SelMode::Face, Act::Mode(SelMode::Face)),
                it(6, "Vertex", m == SelMode::Vertex, Act::Mode(SelMode::Vertex)),
            ]
        }
        Ctx::Empty => vec![
            it(0, "Cube", st.tool == Tool::Create(PrimKind::Box), Act::Place(PrimKind::Box)),
            it(1, "Cylinder", st.tool == Tool::Create(PrimKind::Cylinder), Act::Place(PrimKind::Cylinder)),
            it(2, "Sketch", st.tool == Tool::Create(PrimKind::Profile), Act::Place(PrimKind::Profile)),
            it(3, "Path", st.tool == Tool::Create(PrimKind::Path), Act::Place(PrimKind::Path)),
            it(4, "Frame all", false, Act::FrameAll),
            it(6, "Snap", st.settings.snap, Act::Snap),
        ],
    }
}

fn run(st: &mut AppState, act: Act, ctx: Ctx, at: [f32; 2]) {
    match act {
        Act::Material => {}
        Act::ToggleCarve => {
            if let Ctx::Shape(id) = ctx {
                let op = if st.doc.feature(id).map(|f| f.op == Op::Carve).unwrap_or(false) { Op::Add } else { Op::Carve };
                st.set_op(id, op);
            }
        }
        Act::Frame => st.frame_selection(),
        Act::FrameAll => st.frame_all(),
        Act::Delete => st.delete_selected(),
        Act::Hide => {
            if let Ctx::Shape(id) = ctx {
                st.toggle_visible(id);
                st.selection.retain(|x| *x != id);
            }
        }
        Act::Duplicate => st.duplicate_selected(),
        Act::Fillet | Act::Chamfer => {
            let chamfer = act == Act::Chamfer;
            st.round_chamfer = chamfer;
            if !st.edge_sel.is_empty() {
                st.set_round(1.0, chamfer, true);
                st.end_live();
                st.edge_sel.clear();
            }
        }
        Act::FitScan => {
            st.fit_round_to_scan();
            st.edge_sel.clear();
        }
        Act::Extrude => st.arm_push(crate::state::PushKind::Extrude, Some(at)),
        Act::Inset => st.arm_push(crate::state::PushKind::Inset, Some(at)),
        Act::Deselect => {
            st.edge_sel.clear();
            st.face_sel.clear();
            st.vert_sel.clear();
        }
        Act::ObjectMode => st.set_sel_mode(SelMode::Object),
        Act::Mode(m) => st.set_sel_mode(m),
        Act::Place(k) => st.arm(k),
        Act::Snap => st.settings.snap = !st.settings.snap,
        Act::TurnOff => {
            if let Ctx::Step(id) = ctx {
                st.toggle_op(id);
            }
        }
        Act::Marker => {
            if let Ctx::Step(id) = ctx {
                if let Some(el) = st.owner_of(id) {
                    let n = st.element_ops(el).iter().position(|x| *x == id).map(|i| i + 1).unwrap_or(0);
                    st.set_marker(el, n);
                }
            }
        }
    }
}

/// Open menu.
pub struct Menu {
    pub center: [f32; 2],
    pub pointer: [f32; 2],
    pub hover: Option<u8>,
    pub ctx: Ctx,
    pub t0: f64,
    pub shown: bool,
    /// Material arc: when it opened, and the ball under the pointer.
    pub arc: Option<f64>,
    pub ball: Option<usize>,
}

/// After a flick: the chosen item alone, fading out.
pub struct Flash {
    pub center: [f32; 2],
    pub dir: u8,
    pub label: &'static str,
    pub t: f64,
}

#[derive(Default)]
pub struct Marking {
    pub menu: Option<Menu>,
    pub flash: Option<Flash>,
}

impl Marking {
    pub fn is_open(&self) -> bool {
        self.menu.is_some()
    }
}

fn dir_of(dx: f32, dy: f32) -> u8 {
    let a = dx.atan2(-dy); // 0 = up, clockwise
    let d = (a / (std::f32::consts::PI / 4.0)).round() as i32;
    ((d + 8) % 8) as u8
}

/// Centre of ball `k` of `n` on the material arc (above the menu).
fn ball_at(c: Pos2, k: usize, n: usize) -> Pos2 {
    let t = if n <= 1 { 0.5 } else { k as f32 / (n - 1) as f32 };
    let a = (t - 0.5) * ARC_SPREAD;
    pos2(c.x + a.sin() * ARC_R, c.y - a.cos() * ARC_R * 0.86)
}

/// What is under the pointer; picks it as a click would (keeping what is
/// picked when it is part of it).
fn context_at(st: &mut AppState, p: [f32; 2]) -> Ctx {
    if st.tool != Tool::Select {
        return Ctx::Empty;
    }
    match st.sel_mode {
        SelMode::Edge => {
            if let Some(e) = st.pick_edge(p) {
                let r = st.edge_ref(e);
                let picked = r.map(|r| st.edge_sel.iter().any(|x| x.key() == r.key() && x.at.distance(r.at) < 1e-3)).unwrap_or(false);
                if !picked {
                    st.click_edge(Some(e), false);
                }
                return Ctx::Edge;
            }
        }
        SelMode::Face => {
            if let Some(h) = st.pick_at(p) {
                let picked = st.face_sel.iter().any(|x| st.model_face_of(x) == Some(h.face));
                if !picked {
                    st.click_face(Some(p), false);
                }
                if !st.face_sel.is_empty() {
                    return Ctx::Face;
                }
            }
        }
        _ => {}
    }
    let id = st.pick_path(p).or_else(|| st.pick_at(p).and_then(|h| st.feature_of_face(h.face)));
    match id {
        Some(id) => {
            if !st.selection.contains(&id) {
                st.select(Some(id), false);
            } else {
                // part of a multiple selection: it becomes the active one
                st.selection.retain(|x| *x != id);
                st.selection.push(id);
            }
            if st.doc.feature(id).map(|f| f.is_solid()).unwrap_or(false) {
                Ctx::Shape(id)
            } else {
                Ctx::Step(id)
            }
        }
        None => Ctx::Empty,
    }
}

/// Viewport input, before the tools. Returns true when the menu takes the
/// pointer this frame (the viewport then ignores it).
pub fn input(ctx: &egui::Context, st: &mut AppState, rect: Rect) -> bool {
    let now = st.now;
    let (pos, pressed, down, alt) = ctx.input(|i| {
        (
            i.pointer.interact_pos().or(i.pointer.hover_pos()),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.button_down(egui::PointerButton::Secondary),
            i.modifiers.alt,
        )
    });
    if st.marking.flash.as_ref().map(|f| now - f.t < FLASH).unwrap_or(false) {
        ctx.request_repaint();
    }
    let Some(pos) = pos else { return st.marking.is_open() };
    let p = [pos.x, pos.y];

    if st.marking.menu.is_none() {
        // only over the 3D view itself (not a panel), no gesture under way
        let in_view = rect.contains(pos) && ctx.layer_id_at(pos).map(|l| l.order == Order::Background).unwrap_or(false);
        let busy = st.drag.is_some() || st.creation.is_some() || st.sketch.is_some() || st.push.is_some() || st.round_drag.is_some() || st.face_drag.is_some() || st.marquee.is_some() || matches!(st.tool, Tool::Knife | Tool::Point);
        if pressed && in_view && !alt && !busy {
            let c = context_at(st, p);
            st.marking.menu = Some(Menu { center: p, pointer: p, hover: None, ctx: c, t0: now, shown: false, arc: None, ball: None });
            ctx.request_repaint();
            return true;
        }
        return false;
    }

    // open: follow the pointer, run the item on release
    let menu = st.marking.menu.as_mut().unwrap();
    menu.pointer = p;
    let c = pos2(menu.center[0], menu.center[1]);
    let (dx, dy) = (p[0] - c.x, p[1] - c.y);
    let dist = dx.hypot(dy);
    menu.hover = if dist > DEAD { Some(dir_of(dx, dy)) } else { None };
    if !menu.shown && now - menu.t0 >= DELAY {
        menu.shown = true;
    }
    // the material arc: opens when the pointer reaches the top item
    if matches!(menu.ctx, Ctx::Shape(_)) {
        if menu.arc.is_none() && menu.hover == Some(0) && dist > RADIUS * 0.7 {
            menu.arc = Some(now);
            menu.shown = true;
        }
        if menu.arc.is_some() && dist < DEAD {
            menu.arc = None;
        }
        menu.ball = None;
        if menu.arc.is_some() {
            let n = MATERIALS.len();
            let mut best = (BALL + 8.0, None);
            for k in 0..n {
                let d = ball_at(c, k, n).distance(pos);
                if d < best.0 {
                    best = (d, Some(k));
                }
            }
            menu.ball = best.1;
        }
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.marking.menu = None;
        return true;
    }
    if !down {
        let menu = st.marking.menu.take().unwrap();
        if let (Some(k), Ctx::Shape(id)) = (menu.ball, menu.ctx) {
            st.drop_material(crate::state::DropTarget::Shape(id), k as u32);
        } else if let Some(d) = menu.hover {
            if let Some(item) = items(st, menu.ctx).into_iter().find(|i| i.dir == d && i.act != Act::Material) {
                if !menu.shown {
                    st.marking.flash = Some(Flash { center: menu.center, dir: d, label: item.label, t: now });
                }
                run(st, item.act, menu.ctx, menu.pointer);
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
    let g = p.layout_no_wrap(label.to_string(), medium(12.0), Color32::WHITE);
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

/// The lit item: WZX Engine's yellow.
const LIT: Color32 = Color32::from_rgb(255, 226, 102);

fn draw_pill(p: &Painter, r: Rect, g: std::sync::Arc<egui::Galley>, hot: bool, alpha: f32) {
    let f = |c: Color32| c.gamma_multiply(alpha);
    p.rect(
        r,
        r.height() * 0.5,
        f(if hot { LIT } else { Color32::from_rgba_unmultiplied(34, 36, 42, 236) }),
        Stroke::new(1.0, f(if hot { LIT } else { Color32::from_rgba_unmultiplied(110, 114, 124, 200) })),
        egui::StrokeKind::Inside,
    );
    let col = if hot { Color32::from_rgb(28, 28, 30) } else { Color32::from_rgb(228, 228, 232) };
    p.galley_with_override_text_color(pos2(r.min.x + 9.0, r.center().y - g.size().y * 0.5), g, f(col));
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn draw(ctx: &egui::Context, st: &AppState) {
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("marking-menu")));
    let now = st.now;
    if let Some(f) = &st.marking.flash {
        let t = (now - f.t) as f32;
        if (0.0..FLASH as f32).contains(&t) {
            let alpha = 1.0 - t / FLASH as f32;
            let c = pos2(f.center[0], f.center[1]);
            let (r, g) = pill_rect(&p, c, f.dir, f.label);
            p.line_segment([c, r.center()], Stroke::new(1.2, LIT.gamma_multiply(alpha * 0.8)));
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
    p.circle_filled(c, 4.5, Color32::from_rgba_unmultiplied(236, 236, 226, 200));
    p.circle_stroke(c, DEAD, Stroke::new(1.0, Color32::from_rgba_unmultiplied(236, 236, 226, 70)));
    if m.hover.is_some() {
        p.line_segment([c, pointer], Stroke::new(1.2, LIT.gamma_multiply(0.8)));
    }
    p.text(c + vec2(0.0, DEAD + 6.0), Align2::CENTER_TOP, m.ctx.title(), regular(11.0), Color32::from_rgba_unmultiplied(236, 236, 226, 170));
    for item in items(st, m.ctx) {
        let label = if item.checked { format!("• {}", item.label) } else { item.label.to_string() };
        let (r, g) = pill_rect(&p, c, item.dir, &label);
        let hot = m.hover == Some(item.dir) && m.ball.is_none();
        draw_pill(&p, r, g, hot || (item.act == Act::Material && m.arc.is_some()), 1.0);
    }
    // the material arc: the balls come out one by one
    if let (Some(t0), Ctx::Shape(id)) = (m.arc, m.ctx) {
        let cur = st.doc.feature(id).map(|f| f.material as usize);
        let n = MATERIALS.len();
        for (k, mat) in MATERIALS.iter().enumerate() {
            let e = ease_out(((now - t0 - k as f64 * 0.0125) / 0.09) as f32);
            if e <= 0.0 {
                continue;
            }
            let from = pos2(c.x, c.y - RADIUS * 0.82);
            let q = from + (ball_at(c, k, n) - from) * e;
            let hot = m.ball == Some(k);
            let r = BALL * e * if hot { 1.3 } else { 1.0 };
            p.circle_filled(q + vec2(0.0, 1.5), r + 1.5, Color32::from_black_alpha((90.0 * e) as u8));
            crate::ui::material_ball(&p, q, r, mat);
            let ring = if hot { LIT } else if cur == Some(k) { Color32::WHITE } else { Color32::from_rgba_unmultiplied(110, 114, 124, 200) };
            p.circle_stroke(q, r, Stroke::new(if hot || cur == Some(k) { 2.0 } else { 1.0 }, ring.gamma_multiply(e)));
            if hot {
                p.text(q + vec2(0.0, -r - 6.0), Align2::CENTER_BOTTOM, mat.name, medium(12.0), Color32::from_rgb(236, 236, 226));
            }
        }
    }
}
