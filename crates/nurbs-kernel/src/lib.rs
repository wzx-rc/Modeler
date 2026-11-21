//! Forme NURBS, the part as B-rep solids (Romain, 17 nov. 2026: "repars de
//! zéro, sans une ligne de SDF").
//!
//! - `doc`: the document, an ordered list of features (a primitive, its
//!   frame, Add or Carve), saved as JSON.
//! - `model`: what the kernel sends back: triangles with the B-rep face each
//!   one belongs to, the faces (tag = which feature, which face of it, kind,
//!   plane or axis), the edges (polyline, the faces on each side).
//! - `proto`: the frames between the app and the kernel process.
//! - `client`: runs the kernel process (`forme-nurbs-kernel`), latest request
//!   wins, restarts it after a crash with the failing step skipped.
//! - `build` (feature `occ`): the document built with OpenCascade.

pub mod client;
pub mod doc;
pub mod model;
pub mod proto;
pub mod remesh;

#[cfg(feature = "occ")]
pub mod build;
#[cfg(feature = "occ")]
pub mod quads;

pub use doc::{Array, ArrayKind, Comp, Doc, EdgeRef, FaceRef, Feature, FeatureId, Form, FormKind, Group, Knife, KnifePoint, Loft, Makes, MaterialLook, Mirror, Op, Paint, Prim, Profile, Push, RefMesh, Revolve, Round, Sheet, Shell, Tweak};
pub use model::{Edge, Face, FaceTag, Model, Quads};
