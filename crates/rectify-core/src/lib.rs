//! Forme Rectify's core: a noisy mesh (made by an AI, or scanned) put right
//! by hand, one tool at a time (Romain, 18 nov. 2026, 13 h 27: the automatic
//! version did not work visually; the hand tools fix what they are pointed
//! at). The triangles never change: the tools move vertices.
//!
//! - `line`: a segment dragged along an edge makes it straight and sharp;
//! - `circle`: a round part made truly round, ring by ring;
//! - `edges`: the straight edges between flat faces found everywhere, each
//!   fixed like a Line;
//! - `sym`: the mirror plane, both sides averaged or one copied.

pub mod circle;
pub mod edges;
pub mod line;
pub mod mesh;
pub mod moves;
pub mod sym;

pub use mesh::{Mesh, Surface, Topo, NONE};
pub use moves::Moves;
