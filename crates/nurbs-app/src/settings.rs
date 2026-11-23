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
    /// The navigation island in the view (`nav.rs`: the axis ball, Zoom,
    /// Pan, Frame; the tip of a pen alone turns, moves and zooms the view).
    /// Off by default on the PC (Romain, 22 nov.: the pen navigation was
    /// asked for the iPad), on with a touch screen.
    pub nav: bool,
    /// The Mirror bar under the top bar (its icon there opens it; Romain
    /// 23 nov., 14 h 15: "il faut mettre le mirror de façon beaucoup plus
    /// clair").
    pub mirror_bar: bool,
    /// The plane of symmetry drawn in the view while the Mirror is on (not
    /// only while it is selected).
    pub mirror_plane: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, origin: true, snap: true, step: 1.0, points: true, show_tree: true, help: true, sub_fixed: false, nav: cfg!(target_os = "ios"), mirror_bar: false, mirror_plane: true }
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
