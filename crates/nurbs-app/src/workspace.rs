//! The three workspaces (Romain, 20 nov. 2026: "créer des jolis tabs avec
//! marqué Forme, Sculpt, Poly… chaque tab switch l'interface… la scène de
//! travail elle est la même"): one scene, one outliner, one navigation,
//! Move / Rotate / Scale and the shortcuts shared; each tab brings its own
//! tools and its accent (Forme white, Sculpt a light pastel copper, Poly a
//! pastel sky blue) so you see where you are.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Workspace {
    /// The NURBS part (OpenCascade): shapes, operations, fillets.
    #[default]
    Forme,
    /// Sculpted meshes and their brushes.
    Sculpt,
    /// Polygonal meshes edited by vertices, edges and faces.
    Poly,
}

impl Workspace {
    pub const ALL: [Workspace; 3] = [Workspace::Forme, Workspace::Sculpt, Workspace::Poly];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Forme => "Forme",
            Workspace::Sculpt => "Sculpt",
            Workspace::Poly => "Poly",
        }
    }

    pub fn index(self) -> u8 {
        match self {
            Workspace::Forme => 0,
            Workspace::Sculpt => 1,
            Workspace::Poly => 2,
        }
    }

    pub fn from_index(i: u8) -> Workspace {
        match i {
            1 => Workspace::Sculpt,
            2 => Workspace::Poly,
            _ => Workspace::Forme,
        }
    }

    /// Its colour as a dot on the tabs (the dark theme's accent).
    pub fn swatch(self) -> Color32 {
        match self {
            Workspace::Forme => Color32::from_rgb(0xF2, 0xF3, 0xF5),
            Workspace::Sculpt => Color32::from_rgb(0xEC, 0xB4, 0x8E),
            Workspace::Poly => Color32::from_rgb(0x9C, 0xD2, 0xF4),
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Workspace::Forme => Icon::TabForme,
            Workspace::Sculpt => Icon::TabSculpt,
            Workspace::Poly => Icon::TabPoly,
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Workspace::Forme => "Forme: hard-surface NURBS shapes (OpenCascade), fillets, operations",
            Workspace::Sculpt => "Sculpt: clay meshes shaped with brushes (draw, clay, smooth, grab…)",
            Workspace::Poly => "Poly: polygonal meshes edited by vertices, edges and faces",
        }
    }
}
