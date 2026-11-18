//! Forme Rectify's core: the scan (a mesh made by an AI, or scanned) that
//! the NURBS tools are laid on. The scan itself never changes (Romain,
//! 18 nov. 2026, 16 h 01: the tools that moved its vertices were dropped; a
//! retopology in NURBS instead, semi-automatic on the scan).
//!
//! - `mesh`: the scan (.obj in, millimetres), its topology, picking helpers;
//! - `nurbs`: NURBS curves and surfaces (exact circles);
//! - `circle`: the Circle tool: a round part found under a drag, a NURBS
//!   surface laid on its wall.

pub mod circle;
pub mod mesh;
pub mod nurbs;

pub use mesh::{Mesh, Surface, Topo, NONE};
