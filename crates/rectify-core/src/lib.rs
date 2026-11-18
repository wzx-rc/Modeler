//! Forme Rectify's core: from a noisy mesh (made by an AI, or scanned) back
//! to what it was meant to be. No retopology and no remodelling: the
//! triangles stay, the vertices move onto the shapes the object is made of.
//!
//! The pipeline (Romain, 18 nov. 2026):
//! 1. symmetry: find the mirror plane, average both sides (`symmetry`);
//! 2. smoothing that keeps the edges (`smooth`);
//! 3. regions: each a plane, a cylinder or a sphere (`segment`);
//! 4. regularization: parallel, square, on the object's axes, same radii,
//!    coaxial, mirrored (`regularize`);
//! 5. snap: the vertices onto their shapes, edges where two shapes meet
//!    (`snap`).

pub mod clean;
pub mod edit;
pub mod mesh;
pub mod pipeline;
pub mod prim;
pub mod regularize;
pub mod segment;
pub mod smooth;
pub mod snap;
pub mod symmetry;

pub use mesh::{Mesh, Topo, NONE};
pub use pipeline::{Params, Pipeline, Step};
pub use prim::{Kind, Prim};
