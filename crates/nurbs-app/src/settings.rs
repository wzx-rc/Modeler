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
    /// iPad: Shift / Ctrl on the right, the tool dock on the left and the
    /// scene list on the right (left-handed: the Pencil in the left hand).
    pub left_handed: bool,
    /// iPad: a finger tap never picks (the fingers only move the view, the
    /// Pencil does the work; a double tap still frames).
    pub pencil_only: bool,
    /// iPad: a finger held still this long (seconds) in the view is the
    /// right button (the marking menu).
    pub hold_time: f64,
    /// The Studio interface (Romain 22 nov.: the mock-up D3 as a theme
    /// that changes the whole UI and UX): soft greys, a soft blue accent
    /// (green in the SDF tab, named Soft), the scene on the left, an
    /// inspector on the right, the tools in a dock at the bottom
    /// (`ui::studio`). Else the classic layout (`ui::tablet` on the iPad).
    pub studio: bool,
    /// The SDF objects' mesh, 0 to 3 (Romain 22 nov., 18 h 50: "Pour la
    /// résolution met un slider avec 4 niveau"): Draft, Medium (as
    /// before), Fine, Ultra (`sdf::RES_LEVELS`).
    pub sdf_res: u8,
    /// Frame times shown over the view (Romain 22 nov., 18 h 50: "en NURBS
    /// c'est lent quand on déplace les éléments", on his iPad Pro of 2017).
    pub show_perf: bool,
    /// What the file was written by: 0 before 22 nov. (no field), 1 since
    /// the Studio interface is the iPad's own (Romain 22 nov., 12 h 04:
    /// "implémenter ce thème sur la version iPad"). An older file is
    /// brought up once (`upgrade`), so a choice made after it is kept.
    #[serde(default)]
    pub version: u32,
}

/// The settings' current version (see `Settings::version`).
pub const VERSION: u32 = 1;

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, origin: true, snap: true, step: 1.0, points: true, show_tree: true, help: true, sub_fixed: false, tab_anim: true, left_handed: false, pencil_only: false, hold_time: 0.45, studio: cfg!(target_os = "ios"), sdf_res: 1, show_perf: false, version: VERSION }
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        crate::files::documents_dir().join("settings.json")
    }
    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str::<Settings>(&s).ok()).map(|s| s.upgrade(cfg!(target_os = "ios"))).unwrap_or_default()
    }
    /// A file written by an older version brought up to this one: on the
    /// iPad (`ipad`) the Studio interface turned on once.
    pub fn upgrade(mut self, ipad: bool) -> Settings {
        if self.version < 1 && ipad {
            self.studio = true;
        }
        self.version = self.version.max(VERSION);
        self
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_made_the_ipads_once() {
        // a file from before (no version), Classic chosen or not
        let old: Settings = serde_json::from_str(r#"{ "light": true, "studio": false }"#).unwrap();
        assert_eq!(old.version, 0);
        let ipad = old.clone().upgrade(true);
        assert!(ipad.studio && ipad.light && ipad.version == VERSION);
        let pc = old.upgrade(false);
        assert!(!pc.studio && pc.version == VERSION);
        // Classic chosen again on the iPad after it: kept
        let again: Settings = serde_json::from_str(&serde_json::to_string(&Settings { studio: false, ..ipad }).unwrap()).unwrap();
        assert!(!again.upgrade(true).studio);
    }
}
