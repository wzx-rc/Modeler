//! User preferences, saved in Documents/Forme NURBS/settings.json.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Light theme.
    pub light: bool,
    /// Ground grid.
    pub grid: bool,
    /// World origin on the grid.
    pub origin: bool,
    /// Moves and sizes snap to `step` mm.
    pub snap: bool,
    pub step: f64,
    /// Creation, sketches and moves snap to the part's points of interest
    /// (corners, middles, centres) and line up with them.
    pub points: bool,
    /// Scene list in the inspector.
    pub show_tree: bool,
    /// The help box at the bottom right, open (else folded to its button).
    pub help: bool,
    /// The Sub Outliner (the clicked element's operations): a fixed
    /// container in the left panel, else a floating column beside it.
    pub sub_fixed: bool,
    /// tab_anim: the change of tab animated (colours fading, the pill
    /// sliding, the new tools coming in).
    pub tab_anim: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, origin: true, snap: true, step: 1.0, points: true, show_tree: true, help: true, sub_fixed: false, tab_anim: true }
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        crate::files::documents_dir().join("settings.json")
    }
    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}
