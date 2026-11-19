//! iPad Pro: fingers and Apple Pencil. The only part of the port that is
//! not the PC app: the touches are turned into the mouse and keyboard events
//! the app already reads, so every tool works as on the PC.
//!
//! - **Pencil** = the mouse's left button. When it lifts in the view its
//!   point stays the pointer's (as a mouse left there), so the
//!   pre-selection and the tools' rubber bands stay where it was.
//! - **A finger on a panel** = the left button too (buttons, sliders, lists).
//! - **Fingers in the 3D view** = the camera: one finger orbits, two pan and
//!   pinch to zoom. A quick tap clicks where it is (picks). Held still
//!   0.45 s it is the right button: the marking menu opens there, the
//!   finger flicks to an item and lets go. Two-finger tap: Undo; three-finger
//!   tap: Redo.
//! - **The key bar** (bottom left, `show_bar`): Shift, Ctrl and Alt held by
//!   a finger (the Pencil works meanwhile), or tapped to stay on until
//!   tapped again; Esc, Enter, Delete and Frame (F) as the keys.
//!
//! On the PC it is off; `FORME_TOUCH=1` turns it on (touch screens).

use crate::state::AppState;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Event, Id, Key, Modifiers, Order, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarKey {
    Shift,
    Ctrl,
    Alt,
    Esc,
    Enter,
    Delete,
    Frame,
}

impl BarKey {
    fn label(self) -> &'static str {
        match self {
            BarKey::Shift => "Shift",
            BarKey::Ctrl => "Ctrl",
            BarKey::Alt => "Alt",
            BarKey::Esc => "Esc",
            BarKey::Enter => "Enter",
            BarKey::Delete => "Del",
            BarKey::Frame => "F",
        }
    }
    fn modifier(self) -> Option<usize> {
        match self {
            BarKey::Shift => Some(0),
            BarKey::Ctrl => Some(1),
            BarKey::Alt => Some(2),
            _ => None,
        }
    }
    fn key(self) -> Option<Key> {
        match self {
            BarKey::Esc => Some(Key::Escape),
            BarKey::Enter => Some(Key::Enter),
            BarKey::Delete => Some(Key::Delete),
            BarKey::Frame => Some(Key::F),
            _ => None,
        }
    }
}

const BAR_KEYS: [BarKey; 7] = [BarKey::Shift, BarKey::Ctrl, BarKey::Alt, BarKey::Esc, BarKey::Enter, BarKey::Delete, BarKey::Frame];
/// The bar's height and its gap to the screen's bottom (the left panel
/// stops above it).
pub const BAR_H: f32 = 48.0;
const BAR_EDGE: f32 = 14.0;
/// A finger held this long without moving is the right button.
const LONG_PRESS: f64 = 0.45;
/// Moved this far (points) it is no longer a tap.
const SLOP: f32 = 10.0;
/// Taps of 2 or 3 fingers shorter than this undo / redo.
const TAP: f64 = 0.35;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    /// Drives the egui pointer (the Pencil, a finger on a panel, a finger
    /// held for the marking menu).
    Pointer { button: PointerButton, in_view: bool },
    /// A finger in the 3D view: the camera.
    Camera,
    Bar(BarKey),
    Ignored,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    role: Role,
    start: Pos2,
    pos: Pos2,
    t0: f64,
    moved: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cam {
    Idle,
    /// One finger down, not moved yet: a tap, an orbit or a long press.
    Pending,
    Orbit,
    Pinch { center: Pos2, dist: f32 },
    /// The fingers left after a pinch: nothing until all are up.
    Done,
}

/// The touches' state, kept in `AppState::touch`.
pub struct TouchInput {
    /// Touches are read here (iPad, or FORME_TOUCH=1), the key bar shows.
    pub on: bool,
    fingers: HashMap<u64, Finger>,
    /// The touch driving the egui pointer.
    pointer: Option<u64>,
    /// The fingers on the 3D view, in the order they came.
    cam_ids: Vec<u64>,
    cam: Cam,
    /// The camera gesture: most fingers down at once, when it began, moved.
    gesture: (usize, f64, bool),
    /// Events for this frame, and for the next one (a tap's release, the
    /// pointer gone after a panel was touched).
    events: Vec<Event>,
    next: Vec<Event>,
    /// Shift, Ctrl, Alt: held by these touches, or latched on.
    held: [Option<u64>; 3],
    latched: [bool; 3],
    /// The key bar's keys, laid out by `show_bar` (screen points).
    pub keys: Vec<(Rect, BarKey)>,
    /// The bar key last pressed and when (it flashes).
    flash: Option<(BarKey, f64)>,
    /// The modifiers egui was last told.
    sent_mods: Modifiers,
}

impl Default for TouchInput {
    fn default() -> Self {
        TouchInput { on: false, fingers: HashMap::new(), pointer: None, cam_ids: vec![], cam: Cam::Idle, gesture: (0, 0.0, false), events: vec![], next: vec![], held: [None; 3], latched: [false; 3], keys: vec![], flash: None, sent_mods: Modifiers::NONE }
    }
}

impl TouchInput {
    /// Shift, Ctrl, Alt from the bar.
    pub fn modifiers(&self) -> Modifiers {
        let on = |k: usize| self.held[k].is_some() || self.latched[k];
        Modifiers { shift: on(0), ctrl: on(1), command: on(1), alt: on(2), mac_cmd: false }
    }

    /// Space the left panel leaves at the bottom for the bar.
    pub fn reserve(&self) -> f32 {
        if self.on {
            BAR_H + 10.0
        } else {
            0.0
        }
    }

    /// Something is touching (keep drawing).
    pub fn active(&self) -> bool {
        !self.fingers.is_empty() || !self.next.is_empty() || !self.events.is_empty()
    }

    fn key_at(&self, p: Pos2) -> Option<BarKey> {
        self.keys.iter().find(|(r, _)| r.expand(4.0).contains(p)).map(|(_, k)| *k)
    }

    fn press_key(&mut self, k: Key, mods: Modifiers) {
        for pressed in [true, false] {
            self.events.push(Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: mods });
        }
    }

    /// Hands the frame's events to egui (after `take_egui_input`).
    pub fn apply(&mut self, raw: &mut egui::RawInput) {
        let m = self.modifiers();
        // egui keeps the modifiers from these events (a hardware keyboard's
        // own come through egui-winit too)
        if m != self.sent_mods {
            raw.events.push(Event::ModifiersChanged(m));
            self.sent_mods = m;
        }
        for mut e in self.events.drain(..) {
            match &mut e {
                Event::PointerButton { modifiers, .. } | Event::Key { modifiers, .. } => *modifiers = *modifiers | m,
                _ => {}
            }
            raw.events.push(e);
        }
        self.events = std::mem::take(&mut self.next);
    }
}

/// Is this point over the 3D view (not a panel)? Same test as the marking
/// menu's.
fn in_view(ctx: &egui::Context, p: Pos2) -> bool {
    ctx.layer_id_at(p).map(|l| l.order == Order::Background).unwrap_or(true)
}

fn pointer_down(t: &mut TouchInput, p: Pos2, button: PointerButton) {
    t.events.push(Event::PointerMoved(p));
    t.events.push(Event::PointerButton { pos: p, button, pressed: true, modifiers: Modifiers::NONE });
}

fn pointer_up(t: &mut TouchInput, p: Pos2, button: PointerButton, in_view: bool) {
    t.events.push(Event::PointerMoved(p));
    t.events.push(Event::PointerButton { pos: p, button, pressed: false, modifiers: Modifiers::NONE });
    // on a panel the pointer leaves with the finger (no hover left on a
    // button, no tooltip); in the view it stays, as a mouse would
    if !in_view {
        t.next.push(Event::PointerGone);
    }
}

fn two(fingers: &HashMap<u64, Finger>, ids: &[u64]) -> Option<(Pos2, f32)> {
    let a = fingers.get(ids.first()?)?.pos;
    let b = fingers.get(ids.get(1)?)?.pos;
    Some((pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5), a.distance(b).max(1.0)))
}

/// One touch, in screen points. `pencil`: the Apple Pencil (it reports an
/// altitude angle, fingers do not).
pub fn on_touch(ctx: &egui::Context, st: &mut AppState, id: u64, phase: Phase, p: Pos2, pencil: bool, now: f64) {
    let view_h = st.screen[1].max(1.0);
    let (t, cam) = (&mut st.touch, &mut st.camera);
    match phase {
        Phase::Start => {
            let role = if let Some(k) = t.key_at(p) {
                if let Some(m) = k.modifier() {
                    t.held[m] = Some(id);
                } else if let Some(key) = k.key() {
                    // the key itself, without the bar's modifiers (F with
                    // Ctrl latched would do nothing)
                    t.press_key(key, Modifiers::NONE);
                }
                t.flash = Some((k, now));
                Role::Bar(k)
            } else if pencil {
                // the Pencil takes the pointer from a finger on a panel
                if let Some(other) = t.pointer.take() {
                    if let Some(f) = t.fingers.get_mut(&other) {
                        if let Role::Pointer { button, in_view } = f.role {
                            let at = f.pos;
                            f.role = Role::Ignored;
                            pointer_up(t, at, button, in_view);
                        }
                    }
                }
                t.pointer = Some(id);
                pointer_down(t, p, PointerButton::Primary);
                Role::Pointer { button: PointerButton::Primary, in_view: in_view(ctx, p) }
            } else if in_view(ctx, p) {
                if t.cam_ids.is_empty() {
                    t.gesture = (0, now, false);
                    t.cam = Cam::Pending;
                }
                t.cam_ids.push(id);
                t.gesture.0 = t.gesture.0.max(t.cam_ids.len());
                Role::Camera
            } else if t.pointer.is_none() {
                t.pointer = Some(id);
                pointer_down(t, p, PointerButton::Primary);
                Role::Pointer { button: PointerButton::Primary, in_view: false }
            } else {
                Role::Ignored
            };
            t.fingers.insert(id, Finger { role, start: p, pos: p, t0: now, moved: false });
            if role == Role::Camera && t.cam_ids.len() == 2 {
                // a second finger: pan and zoom (a held menu is let go)
                if let Some((center, dist)) = two(&t.fingers, &t.cam_ids) {
                    t.cam = Cam::Pinch { center, dist };
                }
            }
        }
        Phase::Move => {
            let Some(f) = t.fingers.get_mut(&id) else { return };
            let last = f.pos;
            f.pos = p;
            if f.start.distance(p) > SLOP {
                f.moved = true;
            }
            let (role, moved) = (f.role, f.moved);
            match role {
                Role::Pointer { .. } => t.events.push(Event::PointerMoved(p)),
                Role::Camera => {
                    if moved {
                        t.gesture.2 = true;
                    }
                    match t.cam {
                        Cam::Pending if moved => {
                            t.cam = Cam::Orbit;
                            let d = p - last;
                            cam.orbit(d.x, d.y);
                        }
                        Cam::Orbit => {
                            let d = p - last;
                            cam.orbit(d.x, d.y);
                        }
                        Cam::Pinch { center, dist } => {
                            if let Some((c, d)) = two(&t.fingers, &t.cam_ids) {
                                let m = c - center;
                                cam.pan(m.x, m.y, view_h);
                                cam.zoom(d / dist);
                                t.cam = Cam::Pinch { center: c, dist: d };
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Phase::End | Phase::Cancel => {
            let Some(f) = t.fingers.remove(&id) else { return };
            match f.role {
                Role::Bar(k) => {
                    if let Some(m) = k.modifier() {
                        t.held[m] = None;
                        // a tap latches it on (or off again)
                        if now - f.t0 < 0.3 {
                            t.latched[m] = !t.latched[m];
                        }
                    }
                }
                Role::Pointer { button, in_view } => {
                    if t.pointer == Some(id) {
                        t.pointer = None;
                    }
                    pointer_up(t, f.pos, button, in_view);
                }
                Role::Camera => {
                    t.cam_ids.retain(|i| *i != id);
                    let quick = now - t.gesture.1 < TAP && !t.gesture.2 && phase == Phase::End;
                    if t.cam_ids.is_empty() {
                        match t.gesture.0 {
                            // a tap: a click there (released next frame)
                            1 if t.cam == Cam::Pending && !f.moved && now - f.t0 < LONG_PRESS && t.pointer.is_none() && phase == Phase::End => {
                                pointer_down(t, f.start, PointerButton::Primary);
                                t.next.push(Event::PointerButton { pos: f.start, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
                            }
                            2 if quick => t.press_key(Key::Z, Modifiers { ctrl: true, command: true, ..Default::default() }),
                            3 if quick => t.press_key(Key::Z, Modifiers { ctrl: true, command: true, shift: true, ..Default::default() }),
                            _ => {}
                        }
                        t.cam = Cam::Idle;
                    } else if matches!(t.cam, Cam::Pinch { .. }) {
                        if t.cam_ids.len() >= 2 {
                            if let Some((center, dist)) = two(&t.fingers, &t.cam_ids) {
                                t.cam = Cam::Pinch { center, dist };
                            }
                        } else {
                            t.cam = Cam::Done;
                        }
                    }
                }
                Role::Ignored => {}
            }
        }
    }
}

/// Each frame, before the pass: a finger held still long enough in the view
/// becomes the right button (the marking menu).
pub fn tick(st: &mut AppState, now: f64) {
    let t = &mut st.touch;
    if t.cam != Cam::Pending || t.cam_ids.len() != 1 || t.pointer.is_some() {
        return;
    }
    let id = t.cam_ids[0];
    let Some(f) = t.fingers.get(&id).copied() else { return };
    if f.moved || now - f.t0 < LONG_PRESS {
        return;
    }
    t.cam_ids.clear();
    t.cam = Cam::Idle;
    t.pointer = Some(id);
    pointer_down(t, f.pos, PointerButton::Secondary);
    if let Some(f) = t.fingers.get_mut(&id) {
        f.role = Role::Pointer { button: PointerButton::Secondary, in_view: true };
    }
}

/// The key bar, bottom left: Shift, Ctrl, Alt, then Esc, Enter, Del, F.
/// Its keys are read by `on_touch` (not by egui: a finger holds Shift while
/// the Pencil draws).
pub fn show_bar(ctx: &egui::Context, st: &mut AppState) {
    if !st.touch.on {
        st.touch.keys.clear();
        return;
    }
    let screen = ctx.content_rect();
    let now = st.now;
    let mods = st.touch.modifiers();
    let flash = st.touch.flash.filter(|(_, t)| now - t < 0.18).map(|(k, _)| k);
    if flash.is_some() {
        ctx.request_repaint();
    }
    let latched = st.touch.latched;
    let widths: Vec<f32> = BAR_KEYS.iter().map(|k| if k.modifier().is_some() { 64.0 } else { 50.0 }).collect();
    let gap = 6.0;
    let sep = 12.0;
    let pad = 5.0;
    let w: f32 = widths.iter().sum::<f32>() + gap * (BAR_KEYS.len() - 1) as f32 + sep + pad * 2.0;
    let rect = Rect::from_min_size(pos2(screen.min.x + BAR_EDGE, screen.max.y - BAR_EDGE - BAR_H), vec2(w, BAR_H));
    let mut keys = vec![];
    egui::Area::new(Id::new("touch-bar")).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter();
        p.rect(r, 10.0, tint(PANEL(), 0.96), Stroke::new(1.0, PANEL_BORDER()), StrokeKind::Inside);
        let mut x = r.min.x + pad;
        for (i, k) in BAR_KEYS.iter().enumerate() {
            let kr = Rect::from_min_size(pos2(x, r.min.y + pad), vec2(widths[i], BAR_H - pad * 2.0));
            let on = match k {
                BarKey::Shift => mods.shift,
                BarKey::Ctrl => mods.ctrl,
                BarKey::Alt => mods.alt,
                _ => flash == Some(*k),
            };
            let (bg, fg) = if on { (accent(), on_accent()) } else { (RAISED(), TEXT()) };
            p.rect(kr, 7.0, bg, Stroke::new(1.0, if on { accent_border() } else { DIVIDER() }), StrokeKind::Inside);
            p.text(kr.center(), Align2::CENTER_CENTER, k.label(), semibold(13.0), fg);
            // latched: a small dot under the label
            if k.modifier().map(|m| latched[m]).unwrap_or(false) {
                p.circle_filled(pos2(kr.center().x, kr.max.y - 6.0), 2.0, fg);
            }
            keys.push((kr, *k));
            x += widths[i] + gap;
            if i == 2 {
                p.line_segment([pos2(x + sep * 0.5 - gap * 0.5, r.min.y + 10.0), pos2(x + sep * 0.5 - gap * 0.5, r.max.y - 10.0)], Stroke::new(1.0, DIVIDER()));
                x += sep;
            }
        }
    });
    st.touch.keys = keys;
}
