//! The result of a NURBS build as bytes, so a kernel can run in its own
//! process (forme-occ): a crash there cannot take the app down.
//!
//! Layout (little endian): b"FNM1", then either
//! - 0u8, u32 len, message (UTF-8): an error;
//! - 1u8, u32 shapes, u32 booleans, u64 millis, u32 n_ignored, n × (u32 len,
//!   UTF-8), u32 n_pos, n_pos × 3 f32, n_pos × 3 f32 normals, u32 n_tris,
//!   n_tris × 3 u32, n_tris × u32 material, then (optional) u32 n_lines,
//!   n_lines × (u32 n, n × 3 f32): the edges.

use crate::{NurbsMesh, NurbsModel};

const MAGIC: &[u8; 4] = b"FNM1";

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

pub fn encode(res: &Result<NurbsModel, String>) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    match res {
        Err(e) => {
            out.push(0);
            put_str(&mut out, e);
        }
        Ok(m) => {
            out.push(1);
            put_u32(&mut out, m.shapes as u32);
            put_u32(&mut out, m.booleans as u32);
            out.extend_from_slice(&(m.millis as u64).to_le_bytes());
            put_u32(&mut out, m.ignored.len() as u32);
            for s in &m.ignored {
                put_str(&mut out, s);
            }
            put_u32(&mut out, m.mesh.positions.len() as u32);
            for p in m.mesh.positions.iter().chain(m.mesh.normals.iter()) {
                for c in p {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            put_u32(&mut out, m.mesh.tris.len() as u32);
            for t in &m.mesh.tris {
                for c in t {
                    put_u32(&mut out, *c);
                }
            }
            for mat in &m.mesh.tri_material {
                put_u32(&mut out, *mat);
            }
            put_u32(&mut out, m.edges.len() as u32);
            for l in &m.edges {
                put_u32(&mut out, l.len() as u32);
                for p in l {
                    for c in p {
                        out.extend_from_slice(&c.to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// `None` when the bytes are not a complete result (the kernel died).
pub fn decode(bytes: &[u8]) -> Option<Result<NurbsModel, String>> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != MAGIC {
        return None;
    }
    match r.take(1)?[0] {
        0 => Some(Err(r.string()?)),
        _ => {
            let mut m = NurbsModel { shapes: r.u32()? as usize, booleans: r.u32()? as usize, ..Default::default() };
            m.millis = u64::from_le_bytes(r.take(8)?.try_into().ok()?) as u128;
            for _ in 0..r.u32()? {
                m.ignored.push(r.string()?);
            }
            let n = r.u32()? as usize;
            let mut mesh = NurbsMesh::default();
            for _ in 0..n {
                mesh.positions.push([r.f32()?, r.f32()?, r.f32()?]);
            }
            for _ in 0..n {
                mesh.normals.push([r.f32()?, r.f32()?, r.f32()?]);
            }
            let t = r.u32()? as usize;
            for _ in 0..t {
                mesh.tris.push([r.u32()?, r.u32()?, r.u32()?]);
            }
            for _ in 0..t {
                mesh.tri_material.push(r.u32()?);
            }
            m.mesh = mesh;
            // edges: absent from older kernels
            if let Some(n) = r.u32() {
                for _ in 0..n {
                    let k = r.u32()? as usize;
                    let mut l = Vec::with_capacity(k);
                    for _ in 0..k {
                        l.push([r.f32()?, r.f32()?, r.f32()?]);
                    }
                    m.edges.push(l);
                }
            }
            Some(Ok(m))
        }
    }
}
