//! User preferences kept between sessions, in `settings.json` next to the
//! projects (Documents/Forme on PC).

use crate::files;
use forme_core::{CombineMode, PrimKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The two desktop interfaces. They share the scene, the tools and the
/// renderer; only the screen organisation and the way of working differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiMode {
    /// Viewport first, floating panels, history strip (Plasticity / Spline feel).
    #[default]
    Atelier,
    /// Docked panels, menus, command panel, status bar (3ds Max feel).
    Studio,
}

impl UiMode {
    pub const ALL: [UiMode; 2] = [UiMode::Atelier, UiMode::Studio];

    pub fn label(self) -> &'static str {
        match self {
            UiMode::Atelier => "Atelier",
            UiMode::Studio => "Studio",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            UiMode::Atelier => "Viewport en plein écran, panneaux flottants, historique en bas.",
            UiMode::Studio => "Panneaux ancrés, menus, panneau de commandes et barre d'état, façon 3ds Max.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ui_mode: UiMode,
    /// Every primitive and mode (sphere, capsule, Groove, profile, kit…).
    /// Off: Cube and Cylinder, Add / Carve / Intersect only.
    pub advanced: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { ui_mode: UiMode::Atelier, advanced: false }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        files::documents_dir().join("settings.json")
    }

    /// Reads the saved settings (defaults when missing or unreadable).
    /// `FORME_UI=studio` or `FORME_UI=atelier` overrides the interface.
    pub fn load() -> Settings {
        let mut s: Settings = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        match std::env::var("FORME_UI").map(|v| v.to_lowercase()).as_deref() {
            Ok("studio") => s.ui_mode = UiMode::Studio,
            Ok("atelier") => s.ui_mode = UiMode::Atelier,
            _ => {}
        }
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(Self::path(), text)
    }

    pub fn prim_allowed(&self, k: PrimKind) -> bool {
        self.advanced || matches!(k, PrimKind::Box | PrimKind::Cylinder)
    }

    pub fn placeable(&self) -> Vec<PrimKind> {
        PrimKind::PLACEABLE.into_iter().filter(|k| self.prim_allowed(*k)).collect()
    }

    pub fn modes(&self) -> Vec<CombineMode> {
        CombineMode::ALL
            .into_iter()
            .filter(|m| self.advanced || matches!(m, CombineMode::Add | CombineMode::Carve | CombineMode::Intersect))
            .collect()
    }
}
