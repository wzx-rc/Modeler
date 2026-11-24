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
    /// The rectangle in Vertex, Edge and Face modes also takes what is
    /// behind (backface culling off; Romain 23 nov., 14 h 51), else only
    /// what the eye sees (his 21 nov. ask).
    pub select_through: bool,
    /// The interface's size, 1 as it comes (Romain 23 nov., 17 h 59: "un
    /// slider pour scale l'UI"), from 0.7 to 1.6.
    pub ui_scale: f32,
    /// The move gizmo along the picked components' normal (Maya's Axis
    /// Orientation: Normal; Romain 23 nov., 22 h 05: "un transformTool comme
    /// dans Maya qui déplace une sélection par rapport à ces normal"), else
    /// along the world's axes.
    #[serde(default)]
    pub move_normal: bool,
    /// History off (Maya's construction history off; Romain 23 nov.,
    /// 22 h 05: "mettre une option de history en OFF"): each operation,
    /// once made, folded into its shape (Delete History on it), so a long
    /// scene stays short to build.
    #[serde(default)]
    pub history_off: bool,
    /// The Scale tool's Legacy option (Romain 24 nov., 13 h 09: "ça scale en
    /// essayant de maintenir les formes, comme avant mais en fonctionnel (du
    /// coup un fillet ne va pas se modifier)"): a whole Object scaled puts
    /// the scale into each of its features (a box its sizes, a cylinder its
    /// radius and height; fillets, insets and walls keep their size), never
    /// a scale step as a mesh's.
    #[serde(default)]
    pub scale_legacy: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, origin: true, snap: true, step: 1.0, points: true, show_tree: true, help: true, sub_fixed: false, nav: cfg!(target_os = "ios"), mirror_bar: false, mirror_plane: true, select_through: true, ui_scale: 1.0, move_normal: false, history_off: cfg!(target_os = "ios"), scale_legacy: false }
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
