#![allow(deprecated)]
//! Forme Rectify, the app (Romain, 18 nov. 2026: a third version of the
//! software, from zero, only Forme NURBS's viewport and interface kept, in
//! orange). A scan (a mesh made by an AI, or scanned) with a retopology in
//! NURBS laid on it, semi-automatic; the scan itself never changes. No
//! kernel, no OpenCascade.

pub mod camera;
pub mod draw;
pub mod files;
pub mod icons;
pub mod render;
pub mod state;
pub mod theme;
pub mod ui;
pub mod view;
pub mod viewport;
pub mod widgets;

pub use state::AppState;

/// "Forme Rectify 0.1.0 — rectify · 3f2a1bc · 18/11/2026 05:30".
pub fn version_title() -> String {
    let parts: Vec<&str> = [env!("FORME_GIT_BRANCH"), env!("FORME_GIT_COMMIT"), env!("FORME_GIT_DATE")].into_iter().filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        format!("Forme Rectify {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Forme Rectify {} — {}", env!("CARGO_PKG_VERSION"), parts.join(" · "))
    }
}

/// The whole UI for one frame.
pub fn run(ctx: &egui::Context, st: &mut AppState) {
    theme::set_light(ctx, st.settings.light);
    viewport::show(ctx, st);
    if st.bare {
        let r = ctx.content_rect();
        st.view_rect = [0.0, 0.0, r.width(), r.height()];
        return;
    }
    ui::show(ctx, st);
}
