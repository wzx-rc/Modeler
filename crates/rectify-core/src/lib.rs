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
//!    (`snap`);
//! 6. round pieces: turned parts made truly round (`revolve`);
//! 7. clean-up: never rougher than the AI mesh (`tidy`).
//!
//! The app shows four stages (Romain, 18 nov. 2026, 8 h 46): 0 the scan, 1
//! the algorithm alone (steps 1-6), 2 cleaned (7), 3 the algorithm driven by
//! the edits of the AI pass and the hand tools (`script`, steps 4-7 again).

pub mod clean;
pub mod edit;
pub mod mesh;
pub mod pipeline;
pub mod prim;
pub mod regularize;
pub mod revolve;
pub mod script;
pub mod segment;
pub mod smooth;
pub mod snap;
pub mod symmetry;
pub mod tidy;

pub use mesh::{Mesh, Topo, NONE};
pub use pipeline::{Lane, Params, Pipeline, Stage, Step};
pub use prim::{Kind, Prim};
