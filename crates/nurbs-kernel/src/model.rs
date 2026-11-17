//! What the kernel sends back for a document: the B-rep part as triangles
//! (each one knows its face), its faces and its edges. The app draws it,
//! picks faces and edges on it, and never computes geometry of its own.

use crate::doc::FeatureId;

/// Which face of which feature a B-rep face comes from (carried by
/// OpenCascade's history through every boolean).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceTag {
    pub feature: FeatureId,
    /// Box: 0 +X, 1 −X, 2 top (+Y), 3 bottom (−Y), 4 +Z, 5 −Z.
    /// Cylinder: 0 side, 1 top, 2 bottom.
    pub sub: u16,
}

impl FaceTag {
    pub fn pack(self) -> i64 {
        ((self.feature as i64) << 16) | self.sub as i64
    }
    pub fn unpack(v: i64) -> Option<FaceTag> {
        (v >= 0).then(|| FaceTag { feature: (v >> 16) as FeatureId, sub: (v & 0xffff) as u16 })
    }
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
    pub fn code(self) -> u8 {
        self as u8
    }
    pub fn from_code(c: u8) -> SurfKind {
        match c {
            0 => SurfKind::Plane,
            1 => SurfKind::Cylinder,
            2 => SurfKind::Cone,
            3 => SurfKind::Sphere,
            4 => SurfKind::Torus,
            _ => SurfKind::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub tag: Option<FaceTag>,
    pub kind: SurfKind,
    /// A point in its middle and the outward normal there.
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// A plane: through `point` along `normal`; round faces: their axis.
    pub axis_origin: [f64; 3],
    pub axis_dir: [f64; 3],
    pub radius: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Line,
    Circle,
    Curve,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// Index in the shape's edge map (1-based), what a fillet names.
    pub index: i32,
    /// Faces on each side (indices in `Model::faces`, −1 = none; a seam has
    /// its one face).
    pub faces: [i32; 2],
    pub kind: EdgeKind,
    /// A closed surface's own seam (a cylinder's): not drawn.
    pub seam: bool,
    pub pts: Vec<[f32; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    /// The request this answers.
    pub serial: u64,
    /// Fine meshing (idle) or coarse (while something moves).
    pub fine: bool,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Face of each triangle (index in `faces`).
    pub tri_face: Vec<u32>,
    pub faces: Vec<Face>,
    pub edges: Vec<Edge>,
    /// What could not be made (a boolean that failed, a skipped step…).
    pub errors: Vec<String>,
    /// Booleans made, booleans taken from the cache.
    pub booleans: u32,
    pub cached: u32,
    /// Kernel time (ms).
    pub ms: f32,
    /// The part as quads (when the request asked for them).
    pub quads: Option<Quads>,
}

/// The part as quads (`quads.rs`): positions shared by the faces along
/// their edges, each quad's face (index in `faces`), the faces left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quads {
    pub positions: Vec<[f32; 3]>,
    pub quads: Vec<[u32; 4]>,
    pub quad_face: Vec<u32>,
    /// Faces not converted: (face index, why).
    pub skipped: Vec<(u32, String)>,
}

impl Model {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Bounds of the triangles.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        if self.positions.is_empty() {
            return None;
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        Some((lo, hi))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(64 + self.positions.len() * 24 + self.triangles.len() * 16));
        w.u64(self.serial);
        w.u8(self.fine as u8);
        w.u32(self.positions.len() as u32);
        for p in self.positions.iter().chain(&self.normals) {
            p.iter().for_each(|v| w.f32(*v));
        }
        w.u32(self.triangles.len() as u32);
        for t in &self.triangles {
            t.iter().for_each(|v| w.u32(*v));
        }
        for f in &self.tri_face {
            w.u32(*f);
        }
        w.u32(self.faces.len() as u32);
        for f in &self.faces {
            w.i64(f.tag.map(|t| t.pack()).unwrap_or(-1));
            w.u8(f.kind.code());
            for v in f.point.iter().chain(&f.normal).chain(&f.axis_origin).chain(&f.axis_dir) {
                w.f64(*v);
            }
            w.f64(f.radius);
        }
        w.u32(self.edges.len() as u32);
        for e in &self.edges {
            w.i32(e.index);
            w.i32(e.faces[0]);
            w.i32(e.faces[1]);
            w.u8(match e.kind {
                EdgeKind::Line => 0,
                EdgeKind::Circle => 1,
                EdgeKind::Curve => 2,
            });
            w.u8(e.seam as u8);
            w.u32(e.pts.len() as u32);
            for p in &e.pts {
                p.iter().for_each(|v| w.f32(*v));
            }
        }
        w.u32(self.errors.len() as u32);
        for s in &self.errors {
            w.u32(s.len() as u32);
            w.0.extend_from_slice(s.as_bytes());
        }
        w.u32(self.booleans);
        w.u32(self.cached);
        w.f32(self.ms);
        match &self.quads {
            None => w.u8(0),
            Some(q) => {
                w.u8(1);
                w.u32(q.positions.len() as u32);
                for p in &q.positions {
                    p.iter().for_each(|v| w.f32(*v));
                }
                w.u32(q.quads.len() as u32);
                for (c, f) in q.quads.iter().zip(&q.quad_face) {
                    c.iter().for_each(|v| w.u32(*v));
                    w.u32(*f);
                }
                w.u32(q.skipped.len() as u32);
                for (f, s) in &q.skipped {
                    w.u32(*f);
                    w.u32(s.len() as u32);
                    w.0.extend_from_slice(s.as_bytes());
                }
            }
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Option<Model> {
        let mut r = R { b, at: 0 };
        let mut m = Model { serial: r.u64()?, fine: r.u8()? != 0, ..Default::default() };
        let nv = r.u32()? as usize;
        m.positions = (0..nv).map(|_| r.v3()).collect::<Option<_>>()?;
        m.normals = (0..nv).map(|_| r.v3()).collect::<Option<_>>()?;
        let nt = r.u32()? as usize;
        m.triangles = (0..nt).map(|_| Some([r.u32()?, r.u32()?, r.u32()?])).collect::<Option<_>>()?;
        m.tri_face = (0..nt).map(|_| r.u32()).collect::<Option<_>>()?;
        let nf = r.u32()? as usize;
        for _ in 0..nf {
            let tag = FaceTag::unpack(r.i64()?);
            let kind = SurfKind::from_code(r.u8()?);
            let mut d = [0.0f64; 12];
            for v in d.iter_mut() {
                *v = r.f64()?;
            }
            m.faces.push(Face {
                tag,
                kind,
                point: [d[0], d[1], d[2]],
                normal: [d[3], d[4], d[5]],
                axis_origin: [d[6], d[7], d[8]],
                axis_dir: [d[9], d[10], d[11]],
                radius: r.f64()?,
            });
        }
        let ne = r.u32()? as usize;
        for _ in 0..ne {
            let index = r.i32()?;
            let faces = [r.i32()?, r.i32()?];
            let kind = match r.u8()? {
                0 => EdgeKind::Line,
                1 => EdgeKind::Circle,
                _ => EdgeKind::Curve,
            };
            let seam = r.u8()? != 0;
            let n = r.u32()? as usize;
            let pts = (0..n).map(|_| r.v3()).collect::<Option<_>>()?;
            m.edges.push(Edge { index, faces, kind, seam, pts });
        }
        let nerr = r.u32()? as usize;
        for _ in 0..nerr {
            let n = r.u32()? as usize;
            let s = r.b.get(r.at..r.at + n)?;
            r.at += n;
            m.errors.push(String::from_utf8_lossy(s).into_owned());
        }
        m.booleans = r.u32()?;
        m.cached = r.u32()?;
        m.ms = r.f32()?;
        if r.u8()? == 1 {
            let mut q = Quads::default();
            let n = r.u32()? as usize;
            q.positions = (0..n).map(|_| r.v3()).collect::<Option<_>>()?;
            let n = r.u32()? as usize;
            for _ in 0..n {
                q.quads.push([r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
                q.quad_face.push(r.u32()?);
            }
            let n = r.u32()? as usize;
            for _ in 0..n {
                let f = r.u32()?;
                let len = r.u32()? as usize;
                let s = r.b.get(r.at..r.at + len)?;
                r.at += len;
                q.skipped.push((f, String::from_utf8_lossy(s).into_owned()));
            }
            m.quads = Some(q);
        }
        Some(m)
    }
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl R<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let s = self.b.get(self.at..self.at + N)?;
        self.at += N;
        s.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take::<8>().map(u64::from_le_bytes)
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
    fn v3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = Model {
            serial: 7,
            fine: true,
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            tri_face: vec![0],
            faces: vec![Face { tag: Some(FaceTag { feature: 3, sub: 2 }), kind: SurfKind::Plane, point: [0.1, 0.2, 0.0], normal: [0.0, 0.0, 1.0], axis_origin: [0.1, 0.2, 0.0], axis_dir: [0.0, 0.0, 1.0], radius: 0.0 }],
            edges: vec![Edge { index: 1, faces: [0, -1], kind: EdgeKind::Line, seam: false, pts: vec![[0.0; 3], [1.0, 0.0, 0.0]] }],
            errors: vec!["Cylinder: subtraction failed".into()],
            booleans: 2,
            cached: 1,
            ms: 3.5,
            quads: None,
        };
        assert_eq!(Model::decode(&m.encode()).unwrap(), m);
        let q = Model { quads: Some(Quads { positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], quads: vec![[0, 1, 2, 3]], quad_face: vec![0], skipped: vec![(2, "a face with a hole".into())] }), ..m.clone() };
        assert_eq!(Model::decode(&q.encode()).unwrap(), q);
        assert_eq!(FaceTag::unpack(FaceTag { feature: 70000, sub: 5 }.pack()), Some(FaceTag { feature: 70000, sub: 5 }));
        assert_eq!(FaceTag::unpack(-1), None);
    }
}
