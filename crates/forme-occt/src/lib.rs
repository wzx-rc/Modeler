//! Forme's thin layer over OpenCascade (cpp/forme_occt.cpp): primitives,
//! booleans with their junction edges, fillets and chamfers by edge index,
//! meshing. OpenCascade's errors are caught on the C++ side and come back as
//! `Err(message)`: nothing here aborts the process on a failed operation.
//!
//! Conventions (Forme's): shapes centred on the origin, round shapes along
//! +Y, millimetres. Edges are numbered 1.. in the shape's edge map (each
//! edge once).

use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_int};

#[repr(C)]
struct FShape {
    _p: [u8; 0],
}
#[repr(C)]
struct FMesh {
    _p: [u8; 0],
}

extern "C" {
    fn focc_last_error() -> *const c_char;
    fn focc_free(s: *mut FShape);
    fn focc_box(dx: c_double, dy: c_double, dz: c_double) -> *mut FShape;
    fn focc_cylinder(r: c_double, h: c_double) -> *mut FShape;
    fn focc_sphere(r: c_double) -> *mut FShape;
    fn focc_torus(big: c_double, small: c_double) -> *mut FShape;
    fn focc_prism(xy: *const c_double, smooth: *const c_int, n: c_int, h: c_double) -> *mut FShape;
    fn focc_transform(s: *const FShape, m: *const c_double) -> *mut FShape;
    fn focc_fuse(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_cut(a: *const FShape, b: *const FShape) -> *mut FShape;
    fn focc_section_count(s: *const FShape) -> c_int;
    fn focc_section_edge(s: *const FShape, k: c_int) -> c_int;
    fn focc_edge_count(s: *const FShape) -> c_int;
    fn focc_edge_sample(s: *const FShape, index: c_int, n: c_int, xyz: *mut c_double) -> c_int;
    fn focc_fillet(s: *const FShape, idx: *const c_int, n: c_int, r: c_double) -> *mut FShape;
    fn focc_chamfer(s: *const FShape, idx: *const c_int, n: c_int, d: c_double) -> *mut FShape;
    fn focc_volume(s: *const FShape) -> c_double;
    fn focc_is_valid(s: *const FShape) -> c_int;
    fn focc_mesh(s: *const FShape, defl: c_double, angle: c_double) -> *mut FMesh;
    fn focc_mesh_vertices(m: *const FMesh) -> c_int;
    fn focc_mesh_triangles(m: *const FMesh) -> c_int;
    fn focc_mesh_positions(m: *const FMesh) -> *const f32;
    fn focc_mesh_normals(m: *const FMesh) -> *const f32;
    fn focc_mesh_indices(m: *const FMesh) -> *const u32;
    fn focc_mesh_free(m: *mut FMesh);
}

fn last_error() -> String {
    unsafe {
        let p = focc_last_error();
        if p.is_null() {
            return "OpenCascade error".into();
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        if s.is_empty() {
            "OpenCascade error".into()
        } else {
            s
        }
    }
}

/// A B-rep shape (solid) owned on the C++ side.
pub struct Shape(*mut FShape);

// the C++ side keeps no global state per shape: a shape can move to another thread
unsafe impl Send for Shape {}

impl Drop for Shape {
    fn drop(&mut self) {
        unsafe { focc_free(self.0) }
    }
}

fn shape(p: *mut FShape) -> Result<Shape, String> {
    if p.is_null() {
        Err(last_error())
    } else {
        Ok(Shape(p))
    }
}

/// Kind of an edge's curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Line,
    Circle,
    Curve,
}

/// An edge: its index in the shape, its kind, points along it.
#[derive(Clone, Debug)]
pub struct Edge {
    pub index: i32,
    pub kind: EdgeKind,
    pub pts: Vec<[f64; 3]>,
}

/// Triangles of a shape: per-face vertices (sharp edges stay sharp).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl Shape {
    /// Box of full sizes dx, dy, dz.
    pub fn cuboid(dx: f64, dy: f64, dz: f64) -> Result<Shape, String> {
        shape(unsafe { focc_box(dx, dy, dz) })
    }
    /// Cylinder of radius r and full height h along Y.
    pub fn cylinder(r: f64, h: f64) -> Result<Shape, String> {
        shape(unsafe { focc_cylinder(r, h) })
    }
    pub fn sphere(r: f64) -> Result<Shape, String> {
        shape(unsafe { focc_sphere(r) })
    }
    /// Torus around Y: radius of the ring, radius of the tube.
    pub fn torus(big: f64, small: f64) -> Result<Shape, String> {
        shape(unsafe { focc_torus(big, small) })
    }

    /// Closed outline in the XY plane extruded along Z between -h and +h.
    /// A smooth point is rounded into a quadratic curve between the middles
    /// of its two edges (forme-core's Profile, exact here).
    pub fn prism(points: &[[f64; 2]], smooth: &[bool], h: f64) -> Result<Shape, String> {
        let xy: Vec<f64> = points.iter().flatten().copied().collect();
        let sm: Vec<c_int> = (0..points.len()).map(|i| smooth.get(i).copied().unwrap_or(false) as c_int).collect();
        shape(unsafe { focc_prism(xy.as_ptr(), sm.as_ptr(), points.len() as c_int, h) })
    }

    /// Copy moved by an affine map: 3 rows of 4 (rotation × uniform scale,
    /// a mirror allowed, then translation).
    pub fn transformed(&self, rows: [[f64; 4]; 3]) -> Result<Shape, String> {
        let m: Vec<f64> = rows.iter().flatten().copied().collect();
        shape(unsafe { focc_transform(self.0, m.as_ptr()) })
    }

    pub fn fuse(&self, other: &Shape) -> Result<Shape, String> {
        shape(unsafe { focc_fuse(self.0, other.0) })
    }
    pub fn cut(&self, tool: &Shape) -> Result<Shape, String> {
        shape(unsafe { focc_cut(self.0, tool.0) })
    }

    /// Junction edges made by the boolean that produced this shape.
    pub fn section_edges(&self) -> Vec<i32> {
        let n = unsafe { focc_section_count(self.0) };
        let mut v: Vec<i32> = (0..n).map(|k| unsafe { focc_section_edge(self.0, k) }).filter(|i| *i > 0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Every edge once (degenerated ones, like a sphere's poles, left out),
    /// sampled at `n` points.
    pub fn edges(&self, n: usize) -> Vec<Edge> {
        let count = unsafe { focc_edge_count(self.0) };
        (1..=count).filter_map(|i| self.edge(i, n)).collect()
    }

    pub fn edge(&self, index: i32, n: usize) -> Option<Edge> {
        let n = n.max(2);
        let mut buf = vec![0.0f64; 3 * n];
        let k = unsafe { focc_edge_sample(self.0, index, n as c_int, buf.as_mut_ptr()) };
        let kind = match k {
            0 => EdgeKind::Line,
            1 => EdgeKind::Circle,
            2 => EdgeKind::Curve,
            _ => return None,
        };
        Some(Edge { index, kind, pts: buf.chunks(3).map(|c| [c[0], c[1], c[2]]).collect() })
    }

    /// Rounds the edges (indices in this shape) with radius r, in one
    /// operation. Err when OpenCascade cannot make it or the result is not
    /// a valid solid.
    pub fn fillet(&self, edges: &[i32], r: f64) -> Result<Shape, String> {
        shape(unsafe { focc_fillet(self.0, edges.as_ptr(), edges.len() as c_int, r) })
    }
    pub fn chamfer(&self, edges: &[i32], d: f64) -> Result<Shape, String> {
        shape(unsafe { focc_chamfer(self.0, edges.as_ptr(), edges.len() as c_int, d) })
    }

    pub fn volume(&self) -> f64 {
        unsafe { focc_volume(self.0) }
    }
    pub fn is_valid(&self) -> bool {
        unsafe { focc_is_valid(self.0) == 1 }
    }

    /// Triangles: chord deflection `defl` (mm), angular deflection (rad).
    pub fn mesh(&self, defl: f64, angle: f64) -> Result<Mesh, String> {
        let m = unsafe { focc_mesh(self.0, defl, angle) };
        if m.is_null() {
            return Err(last_error());
        }
        unsafe {
            let nv = focc_mesh_vertices(m).max(0) as usize;
            let nt = focc_mesh_triangles(m).max(0) as usize;
            let p = std::slice::from_raw_parts(focc_mesh_positions(m), nv * 3);
            let n = std::slice::from_raw_parts(focc_mesh_normals(m), nv * 3);
            let t = std::slice::from_raw_parts(focc_mesh_indices(m), nt * 3);
            let out = Mesh {
                positions: p.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
                normals: n.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
                triangles: t.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(),
            };
            focc_mesh_free(m);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_hole_fillet() {
        let cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        let hole = Shape::cylinder(4.0, 28.0).unwrap();
        let cut = cube.cut(&hole).unwrap();
        let sec = cut.section_edges();
        assert_eq!(sec.len(), 2, "two rims");
        let f = cut.fillet(&sec, 1.5).unwrap();
        assert!(f.is_valid());
        let v = f.volume();
        let raw = 8000.0 - std::f64::consts::PI * 16.0 * 20.0;
        assert!(v < raw && v > raw - 60.0, "volume {v}");
        // a fillet too big fails cleanly
        assert!(cut.fillet(&sec, 30.0).is_err());
        let m = f.mesh(0.02, 0.25).unwrap();
        assert!(m.triangles.len() > 100);
    }

    #[test]
    fn profile_prism() {
        // an L, both windings: same positive volume, valid
        let l = [[0.0, 0.0], [20.0, 0.0], [20.0, 6.0], [6.0, 6.0], [6.0, 20.0], [0.0, 20.0]];
        let area = 20.0 * 6.0 + 6.0 * 14.0;
        for pts in [l.to_vec(), l.iter().rev().copied().collect()] {
            let p = Shape::prism(&pts, &[false; 6], 2.0).unwrap();
            assert!(p.is_valid());
            assert!((p.volume() - area * 4.0).abs() < 1e-6, "volume {}", p.volume());
            assert_eq!(p.edges(2).len(), 18);
        }
        // a square with one round corner: a little less matter
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let r = Shape::prism(&sq, &[false, false, true, false], 1.0).unwrap();
        let cut = 25.0 / 6.0; // area between the corner and the curve: (5 x 5) / 6
        assert!((r.volume() - (100.0 - cut) * 2.0).abs() < 1e-6, "volume {}", r.volume());
        // all smooth: a rounded square, one face around
        let o = Shape::prism(&sq, &[true; 4], 1.0).unwrap();
        assert!(o.is_valid() && o.volume() > 0.0);
        // a bow tie crosses itself
        assert!(Shape::prism(&[[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]], &[false; 4], 1.0).is_err());
        // it fillets like the rest
        let p = Shape::prism(&l, &[false; 6], 2.0).unwrap();
        let all: Vec<i32> = p.edges(2).iter().map(|e| e.index).collect();
        assert!(p.fillet(&all, 0.5).unwrap().is_valid());
    }
}
