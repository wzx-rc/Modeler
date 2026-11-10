#![allow(deprecated)]
//! Forme — hard-surface modeller. Shared app code for desktop and iOS.
//! Two desktop interfaces share the state and the renderer: `ui` (Atelier)
//! and `studio` (Studio), chosen in the settings.

/// "Forme 0.1.0 — forme-pc · 3f2a1bc · 10/11/2026 05:30": version, branch,
/// commit and commit date (see build.rs). Shown in the window title.
pub fn version_title() -> String {
    let parts: Vec<&str> = [env!("FORME_GIT_BRANCH"), env!("FORME_GIT_COMMIT"), env!("FORME_GIT_DATE")].into_iter().filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        format!("Forme {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Forme {} — {}", env!("CARGO_PKG_VERSION"), parts.join(" · "))
    }
}

pub mod camera;
pub mod files;
pub mod gfx;
pub mod icons;
pub mod kit;
pub mod settings;
pub mod state;
pub mod studio;
pub mod theme;
pub mod ui;
pub mod viewport;

pub use state::AppState;

/// Runs the whole UI for one frame.
pub fn run(ctx: &egui::Context, st: &mut AppState) {
    viewport::show(ctx, st);
    match st.settings.ui_mode {
        settings::UiMode::Atelier => ui::show(ctx, st),
        settings::UiMode::Studio => studio::show(ctx, st),
    }
    ui::display_panel(ctx, st);
    ui::settings_dialog(ctx, st);
}
