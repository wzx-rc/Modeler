//! A scripted run of the iPad app for Apple's simulator (22 nov., Romain:
//! "prends le temps de bien construire et désigner la version iPad"): with
//! `FORME_SIM=1` the app plays a few steps on its own, as fingers and the
//! Pencil would (taps on the tabs, a shape added, the view under the grid,
//! a tap on the part), and after each one writes its number and name to
//! `Documents/sim-step.txt`; the workflow `ipad-sim.yml` takes a screenshot
//! of the simulator each time it changes. `Documents/sim-log.txt` keeps the
//! window's sizes, the safe area and the state after each step.

use crate::state::AppState;
use crate::touch::Phase;
use egui::{Id, LayerId, Order, Pos2, Rect};
use glam::Vec3;
use std::io::Write;

/// Seconds between two steps (the screenshot is taken half-way).
const STEP: f64 = 4.0;
/// The first step, once the app has started (the kernel, the first frames).
const WARMUP: f64 = 8.0;

pub struct Sim {
    step: usize,
    next: f64,
    marked: Option<(usize, f64)>,
    pub done: bool,
}

const STEPS: [&str; 12] = [
    "empty",
    "tap-soft",
    "bean-added",
    "under-grid",
    "tap-crisp",
    "crisp-settled",
    "tap-capsule-in-crisp",
    "tap-soft-again",
    "tap-capsule-in-soft",
    "block-added",
    "light",
    "end",
];

impl Sim {
    pub fn from_env() -> Option<Sim> {
        (std::env::var("FORME_SIM").map(|v| v == "1").unwrap_or(false)).then_some(Sim { step: 0, next: WARMUP, marked: None, done: false })
    }

    /// Once per frame, before the frame's input is taken: plays the step due.
    /// `info`: the window's sizes, written to the log with each step.
    pub fn tick(&mut self, ctx: &egui::Context, st: &mut AppState, now: f64, info: &str) {
        if self.done {
            return;
        }
        // the step played half a step ago: its picture
        if let Some((k, at)) = self.marked {
            if now >= at {
                self.marked = None;
                let name = STEPS[k];
                let docs = crate::files::documents_dir();
                let _ = std::fs::create_dir_all(&docs);
                let state = format!(
                    "step {k} {name}: workspace {:?}, selection {:?}, sdf {:?}, tool {:?}, mode {:?}, studio {}, content {:?}, viewport and insets {:?}, {info}",
                    st.workspace,
                    st.selection,
                    st.sdf_sel,
                    st.tool,
                    st.sel_mode,
                    st.settings.studio,
                    ctx.content_rect(),
                    ctx.input(|i| (i.viewport().inner_rect, i.safe_area_insets())),
                );
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(docs.join("sim-log.txt")) {
                    let _ = writeln!(f, "{state}");
                }
                let _ = std::fs::write(docs.join("sim-step.txt"), format!("{k} {name}\n"));
                if name == "end" {
                    self.done = true;
                }
            }
        }
        if now < self.next || self.step >= STEPS.len() {
            return;
        }
        let k = self.step;
        self.step += 1;
        self.next = now + STEP;
        self.marked = Some((k, now + STEP * 0.5));
        match STEPS[k] {
            "tap-soft" | "tap-soft-again" => tap_tab(ctx, st, now, false),
            "tap-crisp" => {
                st.camera.pitch = 0.42;
                tap_tab(ctx, st, now, true);
            }
            "bean-added" => st.sdf_add(forme_core::PrimKind::Capsule),
            "under-grid" => {
                st.stop_flight();
                st.camera.pitch = -0.35;
            }
            "tap-capsule-in-crisp" | "tap-capsule-in-soft" => {
                if let Some(p) = capsule_on_screen(st) {
                    tap(ctx, st, now, p, true);
                }
            }
            "block-added" => {
                // Crisp, a block made at once (its tool's second tap)
                st.set_workspace(crate::workspace::Workspace::Forme);
                st.quick_add(nurbs_kernel::doc::PrimKind::Box);
            }
            "light" => st.settings.light = true,
            _ => {}
        }
    }
}

/// A tap: the finger down and up (a Pencil: the pointer).
fn tap(ctx: &egui::Context, st: &mut AppState, now: f64, p: Pos2, pencil: bool) {
    crate::touch::on_touch(ctx, st, 901, Phase::Start, p, pencil, now);
    crate::touch::on_touch(ctx, st, 901, Phase::End, p, pencil, now + 0.08);
}

/// A finger on the Crisp (the leftmost) or the Soft tab, found by its
/// widget in the tabs' island as last drawn.
fn tap_tab(ctx: &egui::Context, st: &mut AppState, now: f64, left: bool) {
    let tabs: Vec<Rect> = ctx.viewport(|v| {
        let layer = LayerId::new(Order::Middle, Id::new("s-tabs"));
        let mut out: Vec<Rect> = v.this_pass.widgets.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
        if out.is_empty() {
            out = v.prev_pass.widgets.get_layer(layer).filter(|w| w.sense.senses_click() && w.enabled).map(|w| w.rect).collect();
        }
        out
    });
    let tabs: Vec<Rect> = tabs.into_iter().filter(|r| r.width() < 200.0).collect();
    let pick = if left { tabs.iter().min_by(|a, b| a.center().x.total_cmp(&b.center().x)) } else { tabs.iter().max_by(|a, b| a.center().x.total_cmp(&b.center().x)) };
    if let Some(r) = pick {
        tap(ctx, st, now, r.center(), false);
    }
}

/// The middle of the first SDF object on screen.
fn capsule_on_screen(st: &AppState) -> Option<Pos2> {
    let f = st.doc.features.iter().find(|f| crate::sdf::is_sdf(f))?;
    let c = f.origin.as_vec3() + Vec3::Y * 6.0;
    let s = st.view().project(c)?;
    Some(egui::pos2(s[0], s[1]))
}
