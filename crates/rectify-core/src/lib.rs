//! Forme Rectify's core: the scan (a mesh made by an AI, or scanned) that
//! the NURBS tools are laid on. The scan itself never changes (Romain,
//! 18 nov. 2026, 16 h 01: the tools that moved its vertices were dropped; a
//! retopology in NURBS instead, semi-automatic on the scan).
//!
//! - `mesh`: the scan (.obj in, millimetres), its topology, picking helpers.

pub mod mesh;

pub use mesh::{Mesh, Surface, Topo, NONE};
