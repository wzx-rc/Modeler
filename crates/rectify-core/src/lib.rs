//! Forme Rectify's core: the scan (a mesh made by an AI, or scanned) that
//! the NURBS tools are laid on. The scan itself never changes (Romain,
//! 18 nov. 2026, 16 h 01: the tools that moved its vertices were dropped; a
//! retopology in NURBS instead; 22 h 30: the Line, Patch, Face and Path
//! tools dropped for a simple Pen, see the app's `body`).
//!
//! - `mesh`: the scan (.obj in, millimetres), its topology, picking helpers;
//! - `nurbs`: NURBS curves and surfaces (exact circles);
//! - `planes`: the scan's flat faces near a point, the plane to draw on;
//! - `circle`: a round part's wall, its profile;
//! - `slot`: a slot, a box or a round part found under a drag, a NURBS
//!   surface laid on its wall;
//! - `fillet`: a fillet's radius read across a sharp edge of a part laid
//!   on the scan (Forme NURBS).

pub mod bvh;
pub mod circle;
pub mod fillet;
pub mod mesh;
pub mod nurbs;
pub mod planes;
pub mod primary;
pub mod slot;

pub use mesh::{Mesh, Surface, Topo, NONE};
