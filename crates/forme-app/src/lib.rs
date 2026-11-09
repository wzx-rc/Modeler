#![allow(deprecated)]
//! Forme — hard-surface modeller. Shared app code for desktop and iOS.
//! Two desktop interfaces share the state and the renderer: `ui` (Atelier)
//! and `studio` (Studio), chosen in the settings.

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
    ui::settings_dialog(ctx, st);
}
