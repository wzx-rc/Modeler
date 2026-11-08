#![allow(deprecated)]
//! Forme — iPad hard-surface modeller. Shared app code for iOS and desktop.

pub mod camera;
pub mod files;
pub mod gfx;
pub mod icons;
pub mod kit;
pub mod state;
pub mod theme;
pub mod ui;
pub mod viewport;

pub use state::AppState;

/// Runs the whole UI for one frame.
pub fn run(ctx: &egui::Context, st: &mut AppState) {
    viewport::show(ctx, st);
    ui::show(ctx, st);
}
