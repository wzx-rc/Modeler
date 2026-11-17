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
#[repr(C)]
struct FLines {
    _p: [u8; 0],
}
#[repr(C)]
struct FBuf {
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
    fn focc_edge_faces(s: *const FShape, out: *mut c_int, cap: c_int) -> c_int;
    fn focc_edge_sample(s: *const FShape, index: c_int, n: c_int, xyz: *mut c_double) -> c_int;
    fn focc_fillet(s: *const FShape, idx: *const c_int, n: c_int, r: c_double, fresh: i64) -> *mut FShape;
    fn focc_chamfer(s: *const FShape, idx: *const c_int, n: c_int, d: c_double, fresh: i64) -> *mut FShape;
    fn focc_face_count(s: *const FShape) -> c_int;
    fn focc_face_info(s: *const FShape, index: c_int, out: *mut c_double) -> c_int;
    fn focc_set_tags(s: *mut FShape, tags: *const i64, n: c_int);
    fn focc_face_tag(s: *const FShape, index: c_int) -> i64;
    fn focc_topo(s: *const FShape, defl: c_double, angle: c_double) -> *mut FBuf;
    fn focc_buf_len(b: *const FBuf) -> c_int;
    fn focc_buf_data(b: *const FBuf) -> *const u8;
    fn focc_buf_free(b: *mut FBuf);
    fn focc_quad_topo(s: *const FShape) -> *mut FBuf;
    fn focc_edge_on_face(s: *const FShape, face: c_int, edge: c_int, reversed: c_int, n: c_int, t: *const c_double, xyz: *mut c_double, uv: *mut c_double) -> c_int;
    fn focc_face_eval(s: *const FShape, face: c_int, n: c_int, uv: *const c_double, out: *mut c_double) -> c_int;
    fn focc_volume(s: *const FShape) -> c_double;
    fn focc_is_valid(s: *const FShape) -> c_int;
    fn focc_mesh(s: *const FShape, defl: c_double, angle: c_double) -> *mut FMesh;
    fn focc_mesh_vertices(m: *const FMesh) -> c_int;
    fn focc_mesh_triangles(m: *const FMesh) -> c_int;
    fn focc_mesh_positions(m: *const FMesh) -> *const f32;
    fn focc_mesh_normals(m: *const FMesh) -> *const f32;
    fn focc_mesh_indices(m: *const FMesh) -> *const u32;
    fn focc_mesh_free(m: *mut FMesh);
    fn focc_lines(s: *const FShape, defl: c_double, angle: c_double) -> *mut FLines;
    fn focc_lines_count(l: *const FLines) -> c_int;
    fn focc_lines_counts(l: *const FLines) -> *const c_int;
    fn focc_lines_points(l: *const FLines) -> c_int;
    fn focc_lines_xyz(l: *const FLines) -> *const c_double;
    fn focc_lines_free(l: *mut FLines);
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

/// Kind of a face's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Other,
}

impl SurfKind {
    fn from_code(k: i32) -> SurfKind {
        match k {
            0 => SurfKind::Plane,
            1 => SurfKind::Cylinder,
            2 => SurfKind::Cone,
            3 => SurfKind::Sphere,
            4 => SurfKind::Torus,
            _ => SurfKind::Other,
        }
    }
}

/// What a face is: a point in its middle and the outward normal there, its
/// axis (a plane's: through that point along the normal) and its radius
/// (round faces).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceInfo {
    pub kind: SurfKind,
    pub point: [f64; 3],
    pub normal: [f64; 3],
    pub axis_origin: [f64; 3],
    pub axis_dir: [f64; 3],
    pub radius: f64,
}

impl FaceInfo {
    fn from_raw(kind: i32, d: &[f64]) -> FaceInfo {
        FaceInfo {
            kind: SurfKind::from_code(kind),
            point: [d[0], d[1], d[2]],
            normal: [d[3], d[4], d[5]],
            axis_origin: [d[6], d[7], d[8]],
            axis_dir: [d[9], d[10], d[11]],
            radius: d[12],
        }
    }
}

/// A face of a `Topo`: its tag (`Shape::set_tags`, -1 = none) and what it is.
#[derive(Clone, Debug)]
pub struct TopoFace {
    pub tag: i64,
    pub info: FaceInfo,
}

/// An edge of a `Topo`: its index in the shape (for fillets), the faces on
/// each side (indices in `Topo::faces`), its kind, whether it is a closed
/// surface's own seam, and a polyline along it.
#[derive(Clone, Debug)]
pub struct TopoEdge {
    pub index: i32,
    pub faces: [i32; 2],
    pub kind: EdgeKind,
    pub seam: bool,
    pub pts: Vec<[f64; 3]>,
}

/// Everything shown and picked: faces, triangles (each with its face) and
/// edges.
#[derive(Clone, Debug, Default)]
pub struct Topo {
    pub faces: Vec<TopoFace>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub tri_face: Vec<u32>,
    pub edges: Vec<TopoEdge>,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let s = self.b.get(self.at..self.at + N)?;
        self.at += N;
        s.try_into().ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_le_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.take::<8>().map(i64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take::<4>().map(f32::from_le_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take::<8>().map(f64::from_le_bytes)
    }
}

/// What a quad mesher walks: each face's wires (the outer one first) as
/// oriented edges, and each edge's range, ends and a sampling.
#[derive(Clone, Debug, Default)]
pub struct QuadTopo {
    pub faces: Vec<QuadFace>,
    /// `edges[i]` is edge i + 1 of the edge map.
    pub edges: Vec<QuadEdge>,
}

#[derive(Clone, Debug)]
pub struct QuadFace {
    pub kind: SurfKind,
    /// Each wire: (edge, reversed) in order around the face.
    pub wires: Vec<Vec<(i32, bool)>>,
}

#[derive(Clone, Debug)]
pub struct QuadEdge {
    /// Parameter range.
    pub t: [f64; 2],
    /// The vertices at t0 and t1 (1-based vertex map, 0 = none).
    pub v: [i32; 2],
    /// 0 line, 1 circle, 2 other.
    pub kind: i32,
    pub radius: f64,
    pub degenerated: bool,
    /// 33 points from t0 to t1, evenly in the parameter.
    pub pts: Vec<[f64; 3]>,
}

fn parse_quad_topo(b: &[u8]) -> Option<QuadTopo> {
    let mut r = Reader { b, at: 0 };
    let mut t = QuadTopo::default();
    let nf = r.u32()? as usize;
    for _ in 0..nf {
        let kind = SurfKind::from_code(r.i32()?);
        let nw = r.u32()? as usize;
        let mut wires = vec![];
        for _ in 0..nw {
            let n = r.u32()? as usize;
            let mut w = vec![];
            for _ in 0..n {
                w.push((r.i32()?, r.i32()? != 0));
            }
            wires.push(w);
        }
        t.faces.push(QuadFace { kind, wires });
    }
    let ne = r.u32()? as usize;
    for _ in 0..ne {
        let tt = [r.f64()?, r.f64()?];
        let v = [r.i32()?, r.i32()?];
        let kind = r.i32()?;
        let radius = r.f64()?;
        let degenerated = r.i32()? != 0;
        let mut pts = Vec::with_capacity(33);
        for _ in 0..33 {
            pts.push([r.f64()?, r.f64()?, r.f64()?]);
        }
        t.edges.push(QuadEdge { t: tt, v, kind, radius, degenerated, pts });
    }
    Some(t)
}

fn parse_topo(b: &[u8]) -> Option<Topo> {
    let mut r = Reader { b, at: 0 };
    let mut t = Topo::default();
    let nf = r.u32()? as usize;
    for _ in 0..nf {
        let tag = r.i64()?;
        let kind = r.i32()?;
        let mut d = [0.0; 13];
        for v in d.iter_mut() {
            *v = r.f64()?;
        }
        t.faces.push(TopoFace { tag, info: FaceInfo::from_raw(kind, &d) });
    }
    let nv = r.u32()? as usize;
    for _ in 0..nv {
        t.positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    for _ in 0..nv {
        t.normals.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let nt = r.u32()? as usize;
    for _ in 0..nt {
        t.triangles.push([r.u32()?, r.u32()?, r.u32()?]);
    }
    for _ in 0..nt {
        t.tri_face.push(r.u32()?);
    }
    let ne = r.u32()? as usize;
    for _ in 0..ne {
        let index = r.i32()?;
        let faces = [r.i32()?, r.i32()?];
        let kind = match r.i32()? {
            0 => EdgeKind::Line,
            1 => EdgeKind::Circle,
            _ => EdgeKind::Curve,
        };
        let seam = r.i32()? != 0;
        let n = r.u32()? as usize;
        let mut pts = Vec::with_capacity(n);
        for _ in 0..n {
            pts.push([r.f64()?, r.f64()?, r.f64()?]);
        }
        t.edges.push(TopoEdge { index, faces, kind, seam, pts });
    }
    Some(t)
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
    /// of its two edges (the Sketch tool's outlines, `nurbs_kernel::doc::Profile`).
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

    /// For every edge (index i at position i - 1): the indices of the two
    /// faces it joins (-1 = none; a seam joins its face to itself, so its
    /// second face is -1).
    pub fn edge_faces(&self) -> Vec<[i32; 2]> {
        let count = unsafe { focc_edge_count(self.0) };
        if count <= 0 {
            return vec![];
        }
        let mut buf = vec![-1 as c_int; 2 * count as usize];
        let n = unsafe { focc_edge_faces(self.0, buf.as_mut_ptr(), buf.len() as c_int) };
        if n < 0 {
            return vec![];
        }
        buf.chunks(2).take(n as usize).map(|c| [c[0], c[1]]).collect()
    }

    /// Rounds the edges (indices in this shape) with radius r, in one
    /// operation. Err when OpenCascade cannot make it or the result is not
    /// a valid solid.
    /// The new (fillet) faces get the tag `fresh`; the others keep theirs.
    pub fn fillet(&self, edges: &[i32], r: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_fillet(self.0, edges.as_ptr(), edges.len() as c_int, r, fresh) })
    }
    pub fn chamfer(&self, edges: &[i32], d: f64, fresh: i64) -> Result<Shape, String> {
        shape(unsafe { focc_chamfer(self.0, edges.as_ptr(), edges.len() as c_int, d, fresh) })
    }

    pub fn face_count(&self) -> usize {
        unsafe { focc_face_count(self.0) }.max(0) as usize
    }

    /// Face `index` (0-based, the shape's face order).
    pub fn face_info(&self, index: usize) -> Option<FaceInfo> {
        let mut d = [0.0f64; 13];
        let k = unsafe { focc_face_info(self.0, index as c_int, d.as_mut_ptr()) };
        (k >= 0).then(|| FaceInfo::from_raw(k, &d))
    }

    /// Names the faces (one tag per face, in face order). Transforms,
    /// booleans and fillets pass each face's tag on to what it becomes.
    pub fn set_tags(&mut self, tags: &[i64]) {
        unsafe { focc_set_tags(self.0, tags.as_ptr(), tags.len() as c_int) }
    }

    pub fn face_tag(&self, index: usize) -> i64 {
        unsafe { focc_face_tag(self.0, index as c_int) }
    }

    /// Meshes the shape (chord `defl` mm, angle `angle` rad) and returns its
    /// faces, triangles and edges.
    /// The faces' wires and the edges, for the quad mesher.
    pub fn quad_topo(&self) -> Result<QuadTopo, String> {
        let b = unsafe { focc_quad_topo(self.0) };
        if b.is_null() {
            return Err(last_error());
        }
        let out = unsafe {
            let bytes = std::slice::from_raw_parts(focc_buf_data(b), focc_buf_len(b).max(0) as usize);
            let t = parse_quad_topo(bytes);
            focc_buf_free(b);
            t
        };
        out.ok_or_else(|| "quad topology buffer cut short".to_string())
    }

    /// Points of an edge (1-based) at parameters `t`: 3D, and the surface
    /// coordinates on face `face` (0-based) where it is used `reversed` or not.
    pub fn edge_on_face(&self, face: usize, edge: i32, reversed: bool, t: &[f64]) -> Result<(Vec<[f64; 3]>, Vec<[f64; 2]>), String> {
        let mut xyz = vec![0.0; t.len() * 3];
        let mut uv = vec![0.0; t.len() * 2];
        let n = unsafe { focc_edge_on_face(self.0, face as c_int, edge, reversed as c_int, t.len() as c_int, t.as_ptr(), xyz.as_mut_ptr(), uv.as_mut_ptr()) };
        if n < 0 {
            return Err(last_error());
        }
        Ok((xyz.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(), uv.chunks(2).map(|c| [c[0], c[1]]).collect()))
    }

    /// Points of face `face` (0-based) at surface coordinates: the point
    /// and the outward normal.
    pub fn face_eval(&self, face: usize, uv: &[[f64; 2]]) -> Result<Vec<([f64; 3], [f64; 3])>, String> {
        let flat: Vec<f64> = uv.iter().flatten().copied().collect();
        let mut out = vec![0.0; uv.len() * 6];
        let n = unsafe { focc_face_eval(self.0, face as c_int, uv.len() as c_int, flat.as_ptr(), out.as_mut_ptr()) };
        if n < 0 {
            return Err(last_error());
        }
        Ok(out.chunks(6).map(|c| ([c[0], c[1], c[2]], [c[3], c[4], c[5]])).collect())
    }

    pub fn topo(&self, defl: f64, angle: f64) -> Result<Topo, String> {
        let b = unsafe { focc_topo(self.0, defl, angle) };
        if b.is_null() {
            return Err(last_error());
        }
        let out = unsafe {
            let bytes = std::slice::from_raw_parts(focc_buf_data(b), focc_buf_len(b).max(0) as usize);
            let t = parse_topo(bytes);
            focc_buf_free(b);
            t
        };
        out.ok_or_else(|| "topology buffer cut short".to_string())
    }

    pub fn volume(&self) -> f64 {
        unsafe { focc_volume(self.0) }
    }
    pub fn is_valid(&self) -> bool {
        unsafe { focc_is_valid(self.0) == 1 }
    }

    /// The edges a person would draw (face borders, fillet boundaries
    /// included; no seams, no poles) as polylines within `defl` mm and
    /// `angle` rad of the true curves.
    pub fn edge_lines(&self, defl: f64, angle: f64) -> Result<Vec<Vec<[f64; 3]>>, String> {
        let l = unsafe { focc_lines(self.0, defl, angle) };
        if l.is_null() {
            return Err(last_error());
        }
        unsafe {
            let n = focc_lines_count(l).max(0) as usize;
            let counts = std::slice::from_raw_parts(focc_lines_counts(l), n);
            let xyz = std::slice::from_raw_parts(focc_lines_xyz(l), focc_lines_points(l).max(0) as usize * 3);
            let mut out = Vec::with_capacity(n);
            let mut at = 0;
            for &c in counts {
                let c = c.max(0) as usize;
                out.push(xyz[at * 3..(at + c) * 3].chunks(3).map(|q| [q[0], q[1], q[2]]).collect());
                at += c;
            }
            focc_lines_free(l);
            Ok(out)
        }
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
        let f = cut.fillet(&sec, 1.5, 99).unwrap();
        assert!(f.is_valid());
        let v = f.volume();
        let raw = 8000.0 - std::f64::consts::PI * 16.0 * 20.0;
        assert!(v < raw && v > raw - 60.0, "volume {v}");
        // a fillet too big fails cleanly
        assert!(cut.fillet(&sec, 30.0, 99).is_err());
        let m = f.mesh(0.02, 0.25).unwrap();
        assert!(m.triangles.len() > 100);
        // lines: the cube's 12 edges, each rim's two fillet boundaries; the
        // fillets' and the hole's seams left out
        let raw = cut.edge_lines(0.02, 0.2).unwrap();
        assert_eq!(raw.len(), 12 + 2, "cube edges + 2 rims");
        let lines = f.edge_lines(0.02, 0.2).unwrap();
        assert_eq!(lines.len(), 12 + 4, "cube edges + 2 boundaries per rim");
        assert!(lines.iter().all(|l| l.len() >= 2));
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
        assert!(p.fillet(&all, 0.5, 99).unwrap().is_valid());
    }

    #[test]
    fn tags_and_topo() {
        // a cube tagged 10..15, a hole tagged 20..22: the holed cube keeps
        // the cube's six tags and gets the hole's side; the fillet faces
        // get their own tag
        let mut cube = Shape::cuboid(20.0, 20.0, 20.0).unwrap();
        assert_eq!(cube.face_count(), 6);
        cube.set_tags(&(10..16).collect::<Vec<i64>>());
        let mut hole = Shape::cylinder(4.0, 28.0).unwrap();
        let n = hole.face_count();
        assert_eq!(n, 3);
        let kinds: Vec<SurfKind> = (0..n).map(|i| hole.face_info(i).unwrap().kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == SurfKind::Cylinder).count(), 1);
        hole.set_tags(&[20, 21, 22]);
        let moved = hole.transformed([[1.0, 0.0, 0.0, 2.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]).unwrap();
        assert_eq!((0..3).map(|i| moved.face_tag(i)).collect::<Vec<_>>(), vec![20, 21, 22]);
        let cut = cube.cut(&moved).unwrap();
        let t = cut.topo(0.05, 0.3).unwrap();
        assert_eq!(t.faces.len(), 7);
        let mut tags: Vec<i64> = t.faces.iter().map(|f| f.tag).collect();
        tags.sort();
        let side = (0..3).map(|i| moved.face_tag(i)).find(|&tg| {
            let k = (0..3).find(|&j| moved.face_tag(j) == tg).unwrap();
            moved.face_info(k).unwrap().kind == SurfKind::Cylinder
        });
        assert_eq!(tags, {
            let mut v: Vec<i64> = (10..16).collect();
            v.push(side.unwrap());
            v.sort();
            v
        });
        // every triangle has a face, every edge two faces (a seam has its
        // one face on both sides)
        assert_eq!(t.tri_face.len(), t.triangles.len());
        assert!(t.tri_face.iter().all(|f| (*f as usize) < t.faces.len()));
        assert!(!t.edges.is_empty());
        assert!(t.edges.iter().all(|e| e.faces[0] >= 0 && (e.seam || e.faces[1] >= 0) && e.pts.len() >= 2));
        // the hole's seam is flagged
        assert_eq!(t.edges.iter().filter(|e| e.seam).count(), 1);
        let f = cut.fillet(&cut.section_edges(), 1.0, 99).unwrap();
        let tf = f.topo(0.05, 0.3).unwrap();
        assert_eq!(tf.faces.iter().filter(|f| f.tag == 99).count(), 2, "one fillet face per rim");
        assert_eq!(tf.faces.iter().filter(|f| f.tag < 0).count(), 0);
        // edge adjacency without meshing agrees with the topology's
        let adj = cut.edge_faces();
        for e in &t.edges {
            assert_eq!(adj[e.index as usize - 1], e.faces, "edge {}", e.index);
        }
    }
}
