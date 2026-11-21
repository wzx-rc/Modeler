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
//!   finger flicks to an item and lets go (the Knife's cut ends); while a
//!   Sketch or Path is drawn it is Enter (closes it, a path ends open). Two-finger tap: Undo; three-finger
//!   tap: Redo. A double tap frames what is under it (empty space: all).
//!   Settings: Pencil only (a finger tap never picks), the hold time,
//!   left-handed (Shift / Ctrl on the right).
//! - **Palm on the glass**: while the Pencil is down, a finger that comes
//!   down in the view is ignored.
//! - **Shift and Ctrl** (`show_bar`, Romain 19 nov.: two round floating
//!   buttons, upright on the left, no Alt: the fingers turn the view): held
//!   by a finger while the Pencil works, or tapped to stay on until tapped
//!   again.
//!
//! On the PC it is off; `FORME_TOUCH=1` turns it on (touch screens).

use crate::state::AppState;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Event, Id, Key, Modifiers, Order, PointerButton, Pos2, Rect, Sense, Stroke};
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
}

impl BarKey {
    fn label(self) -> &'static str {
        match self {
            BarKey::Shift => "Shift",
            BarKey::Ctrl => "Ctrl",
        }
    }
    fn index(self) -> usize {
        match self {
            BarKey::Shift => 0,
            BarKey::Ctrl => 1,
        }
    }
}

/// Bottom to top: Shift under the thumb, Ctrl above it.
const BAR_KEYS: [BarKey; 2] = [BarKey::Shift, BarKey::Ctrl];
/// The round buttons' size, their gap and their margin to the screen's
/// edges (the left panel stops above them).
pub const KEY_D: f32 = 76.0;
pub const KEY_GAP: f32 = 14.0;
pub const BAR_EDGE: f32 = 16.0;
/// Two taps this close in time (seconds) and space (points): a double tap.
const DOUBLE_TAP: f64 = 0.35;
const DOUBLE_SLOP: f32 = 40.0;
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
    pencil: bool,
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
    /// Touches are read here (iPad, or FORME_TOUCH=1), the Shift / Ctrl buttons show.
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
    /// Shift, Ctrl: held by these touches, or latched on.
    held: [Option<u64>; 2],
    latched: [bool; 2],
    /// The Shift / Ctrl buttons, laid out by `show_bar` (screen points).
    pub keys: Vec<(Rect, BarKey)>,
    /// The modifiers egui was last told.
    sent_mods: Modifiers,
    /// The last one-finger tap in the view (time, place): a double tap.
    last_tap: Option<(f64, Pos2)>,
    /// A double tap asked to frame (done by `tick`).
    frame_asked: bool,
}

impl Default for TouchInput {
    fn default() -> Self {
        TouchInput { on: false, fingers: HashMap::new(), pointer: None, cam_ids: vec![], cam: Cam::Idle, gesture: (0, 0.0, false), events: vec![], next: vec![], held: [None; 2], latched: [false; 2], keys: vec![], sent_mods: Modifiers::NONE, last_tap: None, frame_asked: false }
    }
}

impl TouchInput {
    /// Shift, Ctrl from the buttons.
    pub fn modifiers(&self) -> Modifiers {
        let on = |k: usize| self.held[k].is_some() || self.latched[k];
        Modifiers { shift: on(0), ctrl: on(1), command: on(1), alt: false, mac_cmd: false }
    }

    /// Space the left panel leaves at the bottom for the bar.
    pub fn reserve(&self) -> f32 {
        if self.on {
            BAR_KEYS.len() as f32 * (KEY_D + KEY_GAP) + 4.0
        } else {
            0.0
        }
    }

    /// Something is touching (keep drawing).
    pub fn active(&self) -> bool {
        !self.fingers.is_empty() || !self.next.is_empty() || !self.events.is_empty()
    }

    fn key_at(&self, p: Pos2) -> Option<BarKey> {
        self.keys.iter().find(|(r, _)| r.center().distance(p) <= r.width() * 0.5 + 6.0).map(|(_, k)| *k)
    }

    fn press_key(&mut self, k: Key, mods: Modifiers) {
        // the shortcuts read egui's modifiers, not the key event's: they
        // are held for this frame (`apply` puts the real ones back next)
        self.events.push(Event::ModifiersChanged(mods));
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
                // a key pressed with its own modifiers (`press_key`)
                Event::ModifiersChanged(x) => {
                    *x = *x | m;
                    self.sent_mods = *x;
                }
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
    let (pencil_only, long_press) = (st.settings.pencil_only, st.settings.hold_time.clamp(0.2, 1.5));
    let (t, cam) = (&mut st.touch, &mut st.camera);
    // the Pencil is down: a palm (a finger) on the view is not a gesture
    let pencil_down = t.fingers.values().any(|f| f.pencil && matches!(f.role, Role::Pointer { .. }));
    match phase {
        Phase::Start => {
            let role = if let Some(k) = t.key_at(p) {
                t.held[k.index()] = Some(id);
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
            } else if pencil_down && in_view(ctx, p) {
                Role::Ignored
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
            t.fingers.insert(id, Finger { role, pencil, start: p, pos: p, t0: now, moved: false });
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
                    let m = k.index();
                    t.held[m] = None;
                    // a tap latches it on (or off again)
                    if now - f.t0 < 0.3 {
                        t.latched[m] = !t.latched[m];
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
                            // a tap: a click there (released next frame); a
                            // second one close by: frame (not a click)
                            1 if t.cam == Cam::Pending && !f.moved && now - f.t0 < long_press && t.pointer.is_none() && phase == Phase::End => {
                                let double = t.last_tap.is_some_and(|(t0, q)| now - t0 < DOUBLE_TAP && q.distance(f.start) < DOUBLE_SLOP);
                                if double {
                                    t.last_tap = None;
                                    t.frame_asked = true;
                                } else {
                                    t.last_tap = Some((now, f.start));
                                    if !pencil_only {
                                        pointer_down(t, f.start, PointerButton::Primary);
                                        t.next.push(Event::PointerButton { pos: f.start, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
                                    }
                                }
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
    if std::mem::take(&mut st.touch.frame_asked) {
        // on a shape (the first tap picked it): it framed; else all
        st.frame_current();
    }
    let long_press = st.settings.hold_time.clamp(0.2, 1.5);
    let t = &mut st.touch;
    if t.cam != Cam::Pending || t.cam_ids.len() != 1 || t.pointer.is_some() {
        return;
    }
    let id = t.cam_ids[0];
    let Some(f) = t.fingers.get(&id).copied() else { return };
    if f.moved || now - f.t0 < long_press {
        return;
    }
    t.cam_ids.clear();
    t.cam = Cam::Idle;
    // a sketch or path under way: Enter (closes it, or ends a path open),
    // there is no key for it on the iPad
    if st.sketch.is_some() {
        t.press_key(Key::Enter, Modifiers::NONE);
        if let Some(f) = t.fingers.get_mut(&id) {
            f.role = Role::Ignored;
        }
        return;
    }
    t.pointer = Some(id);
    pointer_down(t, f.pos, PointerButton::Secondary);
    if let Some(f) = t.fingers.get_mut(&id) {
        f.role = Role::Pointer { button: PointerButton::Secondary, in_view: true };
    }
}

/// Shift and Ctrl: two round floating buttons, one above the other at the
/// bottom left (Shift under the thumb). Read by `on_touch`, not by egui: a
/// finger holds Shift while the Pencil draws.
pub fn show_bar(ctx: &egui::Context, st: &mut AppState) {
    if !st.touch.on {
        st.touch.keys.clear();
        return;
    }
    let screen = ctx.content_rect();
    let mods = st.touch.modifiers();
    let latched = st.touch.latched;
    let mut keys = vec![];
    // left-handed: on the right, under the other thumb
    let x = if st.settings.left_handed { screen.max.x - BAR_EDGE - KEY_D * 0.5 } else { screen.min.x + BAR_EDGE + KEY_D * 0.5 };
    for (i, k) in BAR_KEYS.iter().enumerate() {
        let c = pos2(x, screen.max.y - BAR_EDGE - KEY_D * 0.5 - i as f32 * (KEY_D + KEY_GAP));
        let r = Rect::from_center_size(c, vec2(KEY_D, KEY_D));
        let on = match k {
            BarKey::Shift => mods.shift,
            BarKey::Ctrl => mods.ctrl,
        };
        egui::Area::new(Id::new(("touch-key", i))).order(Order::Foreground).fixed_pos(r.min).show(ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter();
            let c = r.center();
            let rad = KEY_D * 0.5;
            // a soft shadow, then the button
            p.circle_filled(c + vec2(0.0, 2.0), rad + 1.0, egui::Color32::from_black_alpha(70));
            let (bg, fg, ring) = if on { (accent(), on_accent(), accent_border()) } else { (tint(PANEL(), 0.96), TEXT(), PANEL_BORDER()) };
            p.circle(c, rad, bg, Stroke::new(1.0, ring));
            p.text(c, Align2::CENTER_CENTER, k.label(), semibold(15.0), fg);
            // latched: a small dot under the label
            if latched[k.index()] {
                p.circle_filled(pos2(c.x, c.y + 17.0), 2.5, fg);
            }
        });
        keys.push((r, *k));
    }
    st.touch.keys = keys;
}
