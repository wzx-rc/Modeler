#![allow(deprecated)]
//! Forme NURBS, the app (Romain, 17 nov. 2026: "repars de zéro, sans une
//! ligne de SDF"). The part is OpenCascade's (crate nurbs-kernel, in its
//! own process): the app shows its mesh, picks its B-rep faces, and edits a
//! document of features. Same look as the first Forme NURBS.

pub mod align;
pub mod mirror;
pub mod array;
pub mod shell;
pub mod bools;
pub mod cage;
pub mod camera;
pub mod draw;
pub mod faces;
pub mod files;
pub mod forms;
pub mod groups;
pub mod icons;
pub mod knife;
pub mod history;
pub mod marking;
pub mod materials;
pub mod nav;
pub mod pen;
pub mod pick;
pub mod reference;
pub mod remote;
pub mod render;
pub mod scan;
pub mod settings;
pub mod slot;
pub mod snap;
pub mod state;
pub mod surfaces;
pub mod theme;
pub mod ui;
pub mod view;
pub mod viewport;
pub mod widgets;

pub use state::AppState;

/// "Forme NURBS 0.1.0 — nurbs-zero · 3f2a1bc · 17/11/2026 05:30": version,
/// branch, commit and commit date (see build.rs). Shown in the window title.
pub fn version_title() -> String {
    let parts: Vec<&str> = [env!("FORME_GIT_BRANCH"), env!("FORME_GIT_COMMIT"), env!("FORME_GIT_DATE")].into_iter().filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        format!("Forme NURBS {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Forme NURBS {} — {}", env!("CARGO_PKG_VERSION"), parts.join(" · "))
    }
}

/// The whole UI for one frame.
pub fn run(ctx: &egui::Context, st: &mut AppState) {
    theme::set_light(ctx, st.settings.light);
    // an .obj dropped on the window: the reference mesh to model on
    let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
    if let Some(p) = dropped.iter().find(|p| p.extension().map(|e| e.eq_ignore_ascii_case("obj")).unwrap_or(false)) {
        st.import_reference(&p.to_string_lossy());
    }
    // an .obj chosen with the scene's Import scan button
    if let Some(p) = files::picked_scan() {
        st.import_reference(&p);
    }
    viewport::show(ctx, st);
    ui::show(ctx, st);
    // the camera on its way: the next frame soon
    if st.cam_anim.is_some() {
        ctx.request_repaint();
    }
}
