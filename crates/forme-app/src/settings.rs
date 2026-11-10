//! User preferences kept between sessions, in `settings.json` next to the
//! projects (Documents/Forme on PC).

use crate::files;
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
    /// Creation starts on the face under the pointer (3ds Max AutoGrid),
    /// else on the ground plane. On by default.
    pub snap_faces: bool,
    /// How a primitive is created (creation popover of the tool strip).
    pub creation: Creation,
    /// Viewport and display options (light panel).
    pub display: Display,
    /// Camera navigation with the mouse.
    pub navigation: Navigation,
}

/// Camera navigation with the mouse (Réglages).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Navigation {
    /// Maya: Alt + left orbit, Alt + middle pan, Alt + right zoom; the right
    /// button opens the marking menu, a left drag in empty space selects.
    #[default]
    Maya,
    /// Tripo3D / web viewers: left drag in empty space orbits, right or
    /// middle drag pans, wheel zooms; Shift + left drag selects, a left
    /// press held still opens the marking menu.
    Tripo,
}

impl Navigation {
    pub const ALL: [Navigation; 2] = [Navigation::Maya, Navigation::Tripo];
    pub fn label(self) -> &'static str {
        match self {
            Navigation::Maya => "Maya (Alt)",
            Navigation::Tripo => "Tripo3D",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Navigation::Maya => "Alt + clic gauche : orbite · Alt + molette (clic) : pan · Alt + clic droit : zoom. Clic droit : marking menu, clic gauche glissé dans le vide : rectangle de sélection.",
            Navigation::Tripo => "Clic gauche glissé dans le vide : orbite · clic droit ou molette glissés : pan · molette : zoom. Maj + clic gauche glissé : rectangle de sélection, clic gauche maintenu : marking menu.",
        }
    }
}

/// Creation of a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Creation {
    /// Drag the base, release, then the height (3ds Max).
    #[default]
    Interactive,
    /// One click drops it at default size on the face / grid under the
    /// pointer; then it is sized with its handles.
    Direct,
}

/// Viewport look, set in the light panel next to Réglages.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    pub shadows: bool,
    pub ao: bool,
    /// Samples accumulated when the image is still (1 = off).
    pub aa: u32,
    /// Key light: azimuth and elevation (degrees), intensity.
    pub light_azimuth: f32,
    pub light_elevation: f32,
    pub light_intensity: f32,
    pub exposure: f32,
}

impl Default for Display {
    fn default() -> Self {
        Display { shadows: true, ao: true, aa: 16, light_azimuth: -40.0, light_elevation: 45.0, light_intensity: 1.0, exposure: 1.0 }
    }
}

impl Display {
    /// Unit vector towards the key light.
    pub fn light_dir(&self) -> [f32; 3] {
        let (az, el) = (self.light_azimuth.to_radians(), self.light_elevation.to_radians());
        [el.cos() * az.sin(), el.sin(), el.cos() * az.cos()]
    }
}
impl Default for Settings {
    fn default() -> Self {
        Settings { ui_mode: UiMode::Atelier, snap_faces: true, creation: Creation::Interactive, display: Display::default(), navigation: Navigation::Maya }
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
}
